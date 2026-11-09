//! "Move class": give a class a new fully qualified name (another namespace, optionally another
//! short name) and update the project.
//!
//! - the class's file: its `namespace` line, the class name, and `use` lines for classes of the
//!   old namespace it used without importing them; the file moves to the PSR-4 path of the new
//!   name (or is renamed in place when no autoload rule matches);
//! - other files: `use` lines, fully qualified and relative names, unqualified names that relied
//!   on being in the same namespace (a `use` line is added), and string literals holding the
//!   class name (`'App\Models\User'`, `"App\\Models\\User"`).
//!
//! The client asks for the new name (`phpLsp.moveClass` command) and sends `phpLsp/moveClass`:
//! without `newFqn` the server answers with the class's current name, with it the edit.
//! Only files with a single class-like and an unbraced (or no) namespace can be moved.

use super::references::Target;
use crate::convert::*;
use crate::server::Server;
use lsp_types::*;
use php_index::SymLoc;
use php_syntax::{LineIndex, Span, Stmt, StmtKind};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveClassParams {
    pub uri: String,
    /// Current FQN (from the code action); else the class at `position`.
    pub fqn: Option<String>,
    pub position: Option<Position>,
    pub new_fqn: Option<String>,
}

fn valid_fqn(s: &str) -> bool {
    !s.is_empty()
        && s.split('\\').all(|seg| {
            !seg.is_empty()
                && !seg.as_bytes()[0].is_ascii_digit()
                && seg.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80)
        })
}

fn split_fqn(fqn: &str) -> (&str, &str) {
    match fqn.rfind('\\') {
        Some(i) => (&fqn[..i], &fqn[i + 1..]),
        None => ("", fqn),
    }
}

/// Imports and namespace of one file, read from its syntax tree.
#[derive(Default)]
struct FileScope {
    namespace: String,
    /// End of the `namespace X;` statement.
    ns_end: Option<usize>,
    /// Span of the namespace name.
    ns_name: Option<Span>,
    /// The whole `namespace X;` statement.
    ns_stmt: Option<Span>,
    braced: bool,
    /// (alias lowercase, FQN, item span, statement span, statement has a `{ }` group).
    uses: Vec<(String, String, Span, Span, bool)>,
    /// End of the last top-level `use` statement.
    last_use_end: Option<usize>,
    /// End of a leading `declare(...);`.
    declare_end: Option<usize>,
}

fn file_scope(src: &str, stmts: &[Stmt]) -> FileScope {
    let mut fs = FileScope::default();
    fn scan(src: &str, stmts: &[Stmt], fs: &mut FileScope) {
        for s in stmts {
            match &s.kind {
                StmtKind::Namespace { name, body } => {
                    fs.namespace =
                        name.as_ref().map(|n| n.text.trim_start_matches('\\').to_string()).unwrap_or_default();
                    fs.ns_name = name.as_ref().map(|n| n.span);
                    fs.ns_stmt = Some(s.span);
                    match body {
                        Some(b) => {
                            fs.braced = true;
                            scan(src, b, fs);
                        }
                        None => fs.ns_end = Some(s.span.end as usize),
                    }
                }
                StmtKind::Use(items) => {
                    let grouped = s.span.text(src).contains('{');
                    for it in items.iter().filter(|i| i.kind == php_syntax::UseKind::Class) {
                        let alias = it.alias.clone().unwrap_or_else(|| php_index::last_segment(&it.name).to_string());
                        fs.uses.push((alias.to_ascii_lowercase(), it.name.clone(), it.span, s.span, grouped));
                    }
                    fs.last_use_end = Some(s.span.end as usize);
                }
                StmtKind::Control { kw: "declare", .. } if fs.ns_stmt.is_none() && fs.uses.is_empty() => {
                    fs.declare_end = Some(s.span.end as usize);
                }
                _ => {}
            }
        }
    }
    scan(src, stmts, &mut fs);
    fs
}

/// Where a new `use` line goes, and the text to insert for `fqn`.
fn use_insertion(src: &str, fs: &FileScope, fqn: &str) -> (usize, String) {
    if let Some(e) = fs.last_use_end {
        (e, format!("\nuse {fqn};"))
    } else if let Some(e) = fs.ns_end {
        (e, format!("\n\nuse {fqn};"))
    } else if let Some(e) = fs.declare_end {
        (e, format!("\n\nuse {fqn};"))
    } else {
        let tag_end = src.find("<?php").map(|i| i + 5).unwrap_or(0);
        (tag_end, format!("\n\nuse {fqn};"))
    }
}

/// The whole name written at a reference whose last segment is `ident`: `\App\User`, `Models\User`, `User`.
fn full_name(src: &str, ident: Span) -> Span {
    let b = src.as_bytes();
    let mut s = ident.start as usize;
    while s > 0 && (b[s - 1].is_ascii_alphanumeric() || b[s - 1] == b'_' || b[s - 1] == b'\\' || b[s - 1] >= 0x80) {
        s -= 1;
    }
    Span::new(s, ident.end as usize)
}

/// Edits of one file, keyed by start so that duplicates collapse.
#[derive(Default)]
struct FileEdits {
    edits: BTreeMap<(u32, u32), String>,
    /// FQNs to import.
    imports: Vec<String>,
}

impl FileEdits {
    fn replace(&mut self, s: Span, t: impl Into<String>) {
        self.edits.insert((s.start, s.end), t.into());
    }
    fn import(&mut self, fqn: &str) {
        if !self.imports.iter().any(|i| i.eq_ignore_ascii_case(fqn)) {
            self.imports.push(fqn.to_string());
        }
    }
}

impl Server {
    /// "Move class…" on a class declaration.
    pub(super) fn move_class_action(&mut self, uri: &str, range: Range, out: &mut Vec<CodeActionOrCommand>) {
        let Some(doc) = self.docs.get(uri) else { return };
        if doc.source.is_some() || self.index.is_library(uri) {
            return;
        }
        let offset = offset_of(&doc.text, &doc.lines, range.start);
        let Some(file) = self.index.file_by_uri(uri) else { return };
        let Some(class) =
            file.symbols.iter().find(|s| s.kind.is_class_like() && !s.is_virtual && s.name_span.contains(offset))
        else {
            return;
        };
        if file.symbols.iter().filter(|s| s.kind.is_class_like() && !s.is_virtual).count() != 1 {
            return;
        }
        out.push(CodeActionOrCommand::CodeAction(CodeAction {
            title: "Move class…".into(),
            kind: Some(CodeActionKind::new("refactor.move")),
            command: Some(Command {
                title: "Move class…".into(),
                command: "phpLsp.moveClass".into(),
                arguments: Some(vec![json!(uri), json!(class.fqn.to_string())]),
            }),
            ..Default::default()
        }));
    }

    /// `phpLsp/moveClass`.
    pub fn move_class(&mut self, p: MoveClassParams) -> Result<Value, String> {
        let uri = canon(&lsp_uri(&p.uri));
        let file = self.index.file_by_uri(&uri).ok_or("The file is not indexed yet.")?.clone();
        let fid = self.index.file_id(&uri).ok_or("The file is not indexed yet.")?;
        let classes: Vec<usize> = (0..file.symbols.len())
            .filter(|&i| file.symbols[i].kind.is_class_like() && !file.symbols[i].is_virtual)
            .collect();
        let idx = match (&p.fqn, p.position) {
            (Some(f), _) => {
                classes.iter().copied().find(|&i| file.symbols[i].fqn.eq_ignore_ascii_case(f.trim_start_matches('\\')))
            }
            (None, Some(pos)) => {
                let text = self.text_for(&uri).ok_or("Cannot read the file.")?;
                let off = offset_of(&text, &LineIndex::new(&text), pos);
                classes
                    .iter()
                    .copied()
                    .find(|&i| file.symbols[i].range.contains(off))
                    .or_else(|| (classes.len() == 1).then(|| classes[0]))
            }
            _ => None,
        }
        .ok_or("No class here to move.")?;
        if classes.len() != 1 {
            return Err("Only a file that declares a single class, interface, trait or enum can be moved.".into());
        }
        if self.index.is_library(&uri) {
            return Err("Classes in vendor/ or the built-in stubs cannot be moved.".into());
        }
        let sym = &file.symbols[idx];
        let old_fqn = sym.fqn.to_string();
        let Some(new_fqn) = p.new_fqn.as_deref().map(|s| s.trim().trim_start_matches('\\').to_string()) else {
            return Ok(json!({ "fqn": old_fqn }));
        };
        if !valid_fqn(&new_fqn) {
            return Err(format!("'{new_fqn}' is not a valid class name."));
        }
        if new_fqn == old_fqn {
            return Err("The class already has this name.".into());
        }
        if !new_fqn.eq_ignore_ascii_case(&old_fqn) && self.index.find_class(&new_fqn).is_some() {
            return Err(format!("A class named '{new_fqn}' already exists."));
        }
        let (old_ns, old_short) = split_fqn(&old_fqn);
        let (new_ns, new_short) = split_fqn(&new_fqn);
        let (old_ns, old_short, new_ns, new_short) =
            (old_ns.to_string(), old_short.to_string(), new_ns.to_string(), new_short.to_string());
        let short_changed = old_short != new_short;

        let mut per_file: HashMap<String, FileEdits> = HashMap::new();

        // 1. the class's own file
        let src = self.text_for(&uri).ok_or("Cannot read the file.")?.into_owned();
        let parse = php_syntax::parse(&src);
        let fs = file_scope(&src, &parse.stmts);
        if fs.braced {
            return Err("Files with `namespace X { … }` blocks cannot be moved.".into());
        }
        {
            let own = per_file.entry(uri.clone()).or_default();
            match (fs.ns_name, new_ns.is_empty()) {
                (Some(n), false) => own.replace(n, new_ns.clone()),
                (Some(_), true) => {
                    let s = fs.ns_stmt.unwrap();
                    let line_end =
                        src[s.end as usize..].find('\n').map(|i| s.end as usize + i + 1).unwrap_or(s.end as usize);
                    own.replace(Span::new(s.start as usize, line_end), "");
                }
                (None, false) => {
                    let at = fs.declare_end.unwrap_or_else(|| src.find("<?php").map(|i| i + 5).unwrap_or(0));
                    own.replace(Span::new(at, at), format!("\n\nnamespace {new_ns};"));
                }
                (None, true) => {}
            }
            if short_changed {
                own.replace(sym.name_span, new_short.clone());
            }
            // classes of the old namespace used without a `use` line now need one
            if !old_ns.eq_ignore_ascii_case(&new_ns) {
                let analysis = php_index::analyze(&self.index, &src, &parse.stmts);
                for r in &analysis.refs {
                    let php_index::RefTarget::Class(fqn) = &r.target else { continue };
                    let written = r.span.text(&src);
                    if written.starts_with('\\') || fqn.eq_ignore_ascii_case(&old_fqn) {
                        continue;
                    }
                    let first = written.split('\\').next().unwrap_or("").to_ascii_lowercase();
                    if fs.uses.iter().any(|u| u.0 == first) {
                        continue; // imported (or its first segment is)
                    }
                    let (ns, _) = split_fqn(fqn);
                    let expected = format!("{old_ns}\\{written}");
                    if old_ns.is_empty() || !expected.eq_ignore_ascii_case(fqn) || self.index.find_class(fqn).is_none()
                    {
                        continue;
                    }
                    if written.contains('\\') {
                        own.replace(r.span, format!("\\{fqn}"));
                    } else if !ns.eq_ignore_ascii_case(&new_ns) {
                        own.import(fqn);
                    }
                }
            }
        }

        // 2. references elsewhere (and in the file itself)
        let target =
            Target::Syms { locs: vec![SymLoc { file: fid, idx: idx as u32 }], name: old_short.clone(), kind: sym.kind };
        let hits = self.find_hits(&uri, &target, false, true);
        let mut texts: HashMap<String, (String, FileScope)> = HashMap::new();
        for h in &hits {
            if !texts.contains_key(&h.uri) {
                let Some(t) = self.text_of_candidate(&h.uri) else { continue };
                let parsed = php_syntax::parse_for(&t, php_syntax::blade::is_blade(&h.uri));
                let fsx = file_scope(&t, &parsed.stmts);
                texts.insert(h.uri.clone(), (t, fsx));
            }
            let (t, fsx) = &texts[&h.uri];
            let blade = php_syntax::blade::is_blade(&h.uri);
            let edits = per_file.entry(h.uri.clone()).or_default();
            let full = full_name(t, h.span);
            let written = full.text(t);
            // in a `use` statement?
            if let Some(u) = fsx.uses.iter().find(|u| u.2.start <= full.start && full.end <= u.2.end) {
                let (_, _, item, stmt, grouped) = u;
                if *grouped {
                    // take it out of the group, import it on its own line
                    edits.replace(group_item_removal(t, *item, *stmt), "");
                    edits.import(&new_fqn);
                } else {
                    let lead = if written.starts_with('\\') { "\\" } else { "" };
                    edits.replace(full, format!("{lead}{new_fqn}"));
                }
                continue;
            }
            if written.contains('\\') {
                // `\Old\Ns\Name` or a relative `Ns\Name`
                edits.replace(full, format!("\\{new_fqn}"));
            } else {
                let imported = fsx.uses.iter().find(|u| u.0 == written.to_ascii_lowercase());
                match imported {
                    Some((alias, fqn, ..)) => {
                        // imported without an alias: the name follows the class
                        let plain = php_index::last_segment(fqn).eq_ignore_ascii_case(alias);
                        if short_changed && plain {
                            edits.replace(full, new_short.clone());
                        }
                    }
                    None => {
                        // found through the file's own namespace
                        let own_file = h.uri == uri;
                        if !own_file && !fsx.namespace.eq_ignore_ascii_case(&new_ns) {
                            if blade || new_ns.is_empty() {
                                edits.replace(full, format!("\\{new_fqn}"));
                                continue;
                            }
                            edits.import(&new_fqn);
                        }
                        if short_changed {
                            edits.replace(full, new_short.clone());
                        }
                    }
                }
            }
        }

        // 3. the class name in string literals
        let lower = old_short.to_ascii_lowercase();
        let hash = php_index::name_hash(&lower);
        let single = old_fqn.clone();
        let double = old_fqn.replace('\\', "\\\\");
        for id in self.index.files_with_name(hash) {
            let Some(u) = self.index.file(id).map(|f| f.uri.to_string()) else { continue };
            if self.index.is_library(&u) {
                continue;
            }
            let Some(t) = self.text_of_candidate(&u) else { continue };
            for (needle, rep) in [(single.as_str(), new_fqn.clone()), (double.as_str(), new_fqn.replace('\\', "\\\\"))]
            {
                for q in ['\'', '"'] {
                    for lead in ["", "\\", "\\\\"] {
                        let pat = format!("{q}{lead}{needle}{q}");
                        for (i, _) in t.match_indices(&pat) {
                            let s = Span::new(i + 1 + lead.len(), i + 1 + lead.len() + needle.len());
                            per_file.entry(u.clone()).or_default().replace(s, rep.clone());
                        }
                    }
                }
            }
        }

        // 4. the file's new place
        let old_path = uri_to_path(&uri).ok_or("Bad file URI.")?;
        let stem_matches =
            old_path.file_stem().and_then(|s| s.to_str()).is_some_and(|s| s.eq_ignore_ascii_case(&old_short));
        let new_path: Option<PathBuf> = if stem_matches {
            let p = self.psr4_path(&new_fqn).unwrap_or_else(|| old_path.with_file_name(format!("{new_short}.php")));
            (p != old_path).then_some(p)
        } else {
            None
        };
        if let Some(p) = &new_path {
            if p.exists() && !p.to_string_lossy().eq_ignore_ascii_case(&old_path.to_string_lossy()) {
                return Err(format!("{} already exists.", p.display()));
            }
        }

        // build the workspace edit
        let mut ops: Vec<DocumentChangeOperation> = Vec::new();
        let mut files = 0;
        let mut count = 0;
        let mut uris: Vec<String> = per_file.keys().cloned().collect();
        uris.sort();
        for u in uris {
            let fe = per_file.remove(&u).unwrap();
            let Some(t) = (if u == uri { Some(src.clone()) } else { self.text_of_candidate(&u) }) else { continue };
            let lines = LineIndex::new(&t);
            let mut edits: Vec<TextEdit> = Vec::new();
            for ((s, e), text) in &fe.edits {
                edits.push(TextEdit {
                    range: range(&t, &lines, Span::new(*s as usize, *e as usize)),
                    new_text: text.clone(),
                });
            }
            if !fe.imports.is_empty() {
                let fsx = if u == uri {
                    file_scope(&src, &parse.stmts)
                } else {
                    file_scope(&t, &php_syntax::parse(&t).stmts)
                };
                let mut imports = fe.imports.clone();
                imports.sort_by_key(|a| a.to_ascii_lowercase());
                let (at, _) = use_insertion(&t, &fsx, "");
                let text: String = imports
                    .iter()
                    .enumerate()
                    .map(|(i, f)| {
                        let (_, line) = use_insertion(&t, &fsx, f);
                        if i == 0 {
                            line
                        } else {
                            format!("\nuse {f};")
                        }
                    })
                    .collect();
                let pos = range(&t, &lines, Span::new(at, at));
                edits.push(TextEdit { range: pos, new_text: text });
            }
            if edits.is_empty() {
                continue;
            }
            files += 1;
            count += edits.len();
            ops.push(DocumentChangeOperation::Edit(TextDocumentEdit {
                text_document: OptionalVersionedTextDocumentIdentifier { uri: lsp_uri(&u), version: None },
                edits: edits.into_iter().map(OneOf::Left).collect(),
            }));
        }
        if let Some(p) = &new_path {
            ops.push(DocumentChangeOperation::Op(ResourceOp::Rename(RenameFile {
                old_uri: lsp_uri(&uri),
                new_uri: lsp_uri(&path_to_uri(p)),
                options: Some(RenameFileOptions { overwrite: Some(false), ignore_if_exists: Some(false) }),
                annotation_id: None,
            })));
        }
        log::info!("move class {old_fqn} -> {new_fqn}: {count} edit(s) in {files} file(s)");
        let edit = WorkspaceEdit { document_changes: Some(DocumentChanges::Operations(ops)), ..Default::default() };
        Ok(json!({
            "fqn": old_fqn,
            "edit": edit,
            "newPath": new_path.map(|p| p.to_string_lossy().into_owned()),
        }))
    }

    /// The PSR-4 file of `fqn` in the project's own autoload rules (the longest prefix wins).
    fn psr4_path(&self, fqn: &str) -> Option<PathBuf> {
        let mut best: Option<(usize, PathBuf)> = None;
        for c in &self.composers {
            for e in &c.entries {
                if e.psr0 || e.dir.starts_with(&c.vendor_dir) {
                    continue;
                }
                let prefix = e.prefix.as_str();
                if !fqn.starts_with(prefix) || (!prefix.is_empty() && !prefix.ends_with('\\')) {
                    continue;
                }
                if best.as_ref().is_some_and(|(l, _)| *l >= prefix.len()) {
                    continue;
                }
                let rel = fqn[prefix.len()..].replace('\\', "/");
                best = Some((prefix.len(), e.dir.join(format!("{rel}.php"))));
            }
        }
        best.map(|(_, p)| p)
    }
}

/// The span to delete to take `item` out of a `use A\{B, C};` group (with one comma).
fn group_item_removal(src: &str, item: Span, stmt: Span) -> Span {
    let b = src.as_bytes();
    let (mut s, mut e) = (item.start as usize, item.end as usize);
    // the item's own text may not include `B` alone when it carries the prefix: find the word
    let mut j = e;
    while j < stmt.end as usize && b[j].is_ascii_whitespace() {
        j += 1;
    }
    if j < stmt.end as usize && b[j] == b',' {
        e = j + 1;
        while e < stmt.end as usize && b[e] == b' ' {
            e += 1;
        }
        return Span::new(s, e);
    }
    let mut k = s;
    while k > stmt.start as usize && b[k - 1].is_ascii_whitespace() {
        k -= 1;
    }
    if k > stmt.start as usize && b[k - 1] == b',' {
        s = k - 1;
    }
    Span::new(s, e)
}
