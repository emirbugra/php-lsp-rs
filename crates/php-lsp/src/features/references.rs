//! Find references, document highlight and rename.
//!
//! Strategy (same idea as PhpStorm's word index + verification):
//! 1. every indexed file carries a sorted set of the names it declares / mentions;
//! 2. candidates = files containing the target's name;
//! 3. candidates are parsed + analysed in parallel and each reference is resolved against the index.

use crate::convert::*;
use crate::server::Server;
use lsp_types::*;
use php_index::{name_hash, types::Types, MemberKind, RefTarget, SymLoc, SymbolKind};
use php_syntax::{LineIndex, Span};
use rayon::prelude::*;
use serde_json::Value;
use std::collections::HashMap;

pub enum Target {
    Syms { locs: Vec<SymLoc>, name: String, kind: SymbolKind },
    Var { name: String, decl: Span },
}

pub struct Hit {
    pub uri: String,
    pub span: Span,
    pub range: Range,
}

/// The renameable part of a reference: last segment of a qualified name, without the `$`.
pub fn ident_span(src: &str, s: Span) -> Span {
    let t = s.text(src);
    let mut off = 0;
    if let Some(i) = t.rfind('\\') {
        off = i + 1;
    }
    if t[off..].starts_with('$') {
        off += 1;
    }
    Span { start: s.start + off as u32, end: s.end }
}

fn ref_name(t: &RefTarget) -> Option<String> {
    Some(match t {
        RefTarget::Class(f) => php_index::last_segment(f).to_string(),
        RefTarget::Function(c) | RefTarget::Const(c) => php_index::last_segment(c.last()?).to_string(),
        RefTarget::Member { name, .. } => name.clone(),
        RefTarget::Variable { .. } => return None,
    })
}

struct Scan<'a> {
    index: &'a php_index::Index,
    targets: &'a [SymLoc],
    name: &'a str,
    include_decl: bool,
    verify_text: bool,
}

impl<'a> Scan<'a> {
    /// `seed`: variables the file starts with (a Blade template's view data).
    fn run(&self, uri: &str, text: &str, parse: Option<&php_syntax::Parse>, seed: &[(String, Types)]) -> Vec<Span> {
        // cheap pre-filter on the raw text
        let nb = self.name.as_bytes();
        if !text.as_bytes().windows(nb.len()).any(|w| w.eq_ignore_ascii_case(nb)) {
            return Vec::new();
        }
        let owned;
        let parse = match parse {
            Some(p) => p,
            None => {
                owned = php_syntax::parse_for(text, php_syntax::blade::is_blade(uri));
                &owned
            }
        };
        let analysis = php_index::analyze_with(self.index, text, &parse.stmts, seed, &[]);
        let mut out = Vec::new();
        for r in &analysis.refs {
            match ref_name(&r.target) {
                Some(n) if n.eq_ignore_ascii_case(self.name) => {}
                _ => continue,
            }
            if !self.index.resolve(&r.target).iter().any(|l| self.targets.contains(l)) {
                continue;
            }
            let sp = ident_span(text, r.span);
            if self.verify_text && !sp.text(text).eq_ignore_ascii_case(self.name) {
                continue;
            }
            out.push(sp);
        }
        if self.include_decl {
            if let (Some(fid), Some(file)) = (self.index.file_id(uri), self.index.file_by_uri(uri)) {
                for (i, s) in file.symbols.iter().enumerate() {
                    if !s.is_virtual && self.targets.contains(&SymLoc { file: fid, idx: i as u32 }) {
                        out.push(ident_span(text, s.name_span));
                    }
                }
            }
        }
        out.sort_by_key(|s| s.start);
        out.dedup();
        out
    }
}

impl Server {
    /// What is under the cursor, plus the renameable span.
    pub(crate) fn target_at(&mut self, uri: &str, offset: usize) -> Option<(Target, Span)> {
        let analysis = self.analyze_doc(uri)?;
        let text = self.docs.get(uri)?.text.clone();
        if let Some(r) = analysis.ref_at(offset) {
            let origin = ident_span(&text, r.span);
            match &r.target {
                RefTarget::Variable { name, decl: Some(d), .. } => {
                    return Some((Target::Var { name: name.clone(), decl: *d }, origin));
                }
                RefTarget::Variable { .. } => return None,
                t => {
                    let mut locs = self.index.resolve(t);
                    if locs.is_empty() {
                        let missing: Vec<String> = match t {
                            RefTarget::Class(f) => vec![f.clone()],
                            RefTarget::Member { classes, .. } => classes.clone(),
                            _ => Vec::new(),
                        };
                        let mut loaded = false;
                        for c in missing {
                            loaded |= self.autoload(php_index::strip_generic(&c));
                        }
                        if loaded {
                            locs = self.index.resolve(t);
                        }
                    }
                    let first = *locs.first()?;
                    let s = self.index.sym(first);
                    return Some((Target::Syms { name: s.name.to_string(), kind: s.kind, locs }, origin));
                }
            }
        }
        // declaration under the cursor
        let fid = self.index.file_id(uri)?;
        let file = self.index.file_by_uri(uri)?;
        for (i, s) in file.symbols.iter().enumerate() {
            if !s.is_virtual && !s.name_span.is_empty() && s.name_span.contains(offset) {
                let origin = ident_span(&text, s.name_span);
                return Some((
                    Target::Syms {
                        locs: vec![SymLoc { file: fid, idx: i as u32 }],
                        name: s.name.to_string(),
                        kind: s.kind,
                    },
                    origin,
                ));
            }
        }
        None
    }

    /// Text to search for references: an open document's, or the file's (a Blade template's PHP code).
    pub(crate) fn text_of_candidate(&self, uri: &str) -> Option<String> {
        if let Some(d) = self.docs.get(uri) {
            return Some(d.text.clone());
        }
        std::fs::metadata(uri_to_path(uri)?).ok().filter(|m| m.len() < 4 * 1024 * 1024)?;
        let text = read_text(&uri_to_path(uri)?)?;
        Some(if php_syntax::blade::is_blade(uri) { php_syntax::blade::to_php(&text) } else { text })
    }

    /// All references to `target`. `for_rename` widens members to overrides and skips vendor files.
    pub(crate) fn find_hits(&self, uri: &str, target: &Target, include_decl: bool, for_rename: bool) -> Vec<Hit> {
        self.find_hits_in(uri, target, include_decl, for_rename, &|_| true)
    }

    /// [`Self::find_hits`] limited to the candidate files (URIs) `keep` accepts.
    pub(crate) fn find_hits_in(
        &self,
        uri: &str,
        target: &Target,
        include_decl: bool,
        for_rename: bool,
        keep: &(dyn Fn(&str) -> bool + Sync),
    ) -> Vec<Hit> {
        match target {
            Target::Var { name, decl } => {
                let Some(doc) = self.docs.get(uri) else { return Vec::new() };
                let Some(analysis) = self.analyze_doc(uri) else { return Vec::new() };
                let mut spans: Vec<Span> = analysis
                    .refs
                    .iter()
                    .filter(|r| matches!(&r.target, RefTarget::Variable { name: n, decl: Some(d), .. } if n == name && d == decl))
                    .filter(|r| include_decl || r.span != *decl)
                    .map(|r| ident_span(&doc.text, r.span))
                    .collect();
                spans.sort_by_key(|s| s.start);
                spans.dedup();
                spans
                    .into_iter()
                    .map(|sp| Hit { uri: uri.to_string(), span: sp, range: range(&doc.text, &doc.lines, sp) })
                    .collect()
            }
            Target::Syms { locs, name, kind } => {
                let hash = name_hash(name);
                let mut cands: Vec<String> = self
                    .index
                    .files_with_name(hash)
                    .into_iter()
                    .filter_map(|id| self.index.file(id).map(|f| f.uri.to_string()))
                    .filter(|u| keep(u))
                    .collect();
                let target_in_vendor =
                    locs.first().map(|l| self.index.is_library(self.index.uri_of(*l))).unwrap_or(false);
                if for_rename && !target_in_vendor {
                    cands.retain(|u| !self.index.is_library(u));
                }
                cands.sort_by_key(|u| self.index.is_library(u));
                let started = std::time::Instant::now();

                let mut targets = locs.clone();
                if for_rename && matches!(kind, SymbolKind::Method | SymbolKind::Property | SymbolKind::ClassConst) {
                    self.expand_related_members(&cands, locs, name, *kind, &mut targets);
                }

                let scan = Scan { index: &self.index, targets: &targets, name, include_decl, verify_text: for_rename };
                let per_file: Vec<(String, Vec<Hit>)> = self.pool.install(|| {
                    cands
                        .par_iter()
                        .filter_map(|u| {
                            let (text, parse) = match self.docs.get(u) {
                                Some(d) => (std::borrow::Cow::Borrowed(d.text.as_str()), Some(d.parse.clone())),
                                None => (std::borrow::Cow::Owned(self.text_of_candidate(u)?), None),
                            };
                            let seed = self.view_variables(u);
                            let spans = scan.run(u, &text, parse.as_deref(), &seed);
                            if spans.is_empty() {
                                return None;
                            }
                            let lines = LineIndex::new(&text);
                            let hits = spans
                                .into_iter()
                                .map(|sp| Hit { uri: u.clone(), span: sp, range: range(&text, &lines, sp) })
                                .collect();
                            Some((u.clone(), hits))
                        })
                        .collect()
                });
                let secs = started.elapsed().as_secs_f64();
                if secs > 1.0 {
                    log::warn!("references: scanned {} candidate files for '{name}' in {secs:.1} s", cands.len());
                }
                let mut out: Vec<Hit> = per_file.into_iter().flat_map(|(_, h)| h).collect();
                out.sort_by(|a, b| a.uri.cmp(&b.uri).then(a.span.start.cmp(&b.span.start)));
                out
            }
        }
    }

    /// Methods/properties/constants of the same name in related classes (overrides, implementations).
    fn expand_related_members(
        &self,
        cands: &[String],
        locs: &[SymLoc],
        name: &str,
        kind: SymbolKind,
        out: &mut Vec<SymLoc>,
    ) {
        let containers: Vec<String> =
            locs.iter().filter_map(|l| self.index.sym(*l).container.map(|c| c.to_string())).collect();
        if containers.is_empty() {
            return;
        }
        for u in cands {
            let (Some(fid), Some(file)) = (self.index.file_id(u), self.index.file_by_uri(u)) else { continue };
            for (i, s) in file.symbols.iter().enumerate() {
                if s.kind != kind || s.is_virtual {
                    continue;
                }
                let same = if kind == SymbolKind::Method { s.name.eq_ignore_ascii_case(name) } else { s.name == name };
                if !same {
                    continue;
                }
                let Some(c) = &s.container else { continue };
                let related = containers.iter().any(|t| {
                    t.eq_ignore_ascii_case(c) || self.index.is_subclass_of(c, t) || self.index.is_subclass_of(t, c)
                });
                let loc = SymLoc { file: fid, idx: i as u32 };
                if related && !out.contains(&loc) {
                    out.push(loc);
                }
            }
        }
        let _ = MemberKind::Method;
    }

    pub fn references(&mut self, p: ReferenceParams) -> Option<Value> {
        let uri = canon(&p.text_document_position.text_document.uri);
        let offset = {
            let doc = self.docs.get(&uri)?;
            offset_of(&doc.text, &doc.lines, p.text_document_position.position)
        };
        let (target, _) = self.target_at(&uri, offset)?;
        let started = std::time::Instant::now();
        let hits = self.find_hits(&uri, &target, p.context.include_declaration, false);
        log::debug!("references: {} hit(s) in {:.1} ms", hits.len(), started.elapsed().as_secs_f64() * 1000.0);
        let locs: Vec<Location> = hits.into_iter().map(|h| Location { uri: lsp_uri(&h.uri), range: h.range }).collect();
        serde_json::to_value(locs).ok()
    }

    pub fn document_highlight(&mut self, p: DocumentHighlightParams) -> Option<Value> {
        let uri = canon(&p.text_document_position_params.text_document.uri);
        let offset = {
            let doc = self.docs.get(&uri)?;
            offset_of(&doc.text, &doc.lines, p.text_document_position_params.position)
        };
        let (target, _) = self.target_at(&uri, offset)?;
        let doc = self.docs.get(&uri)?;
        let spans: Vec<Span> = match &target {
            Target::Var { .. } => self.find_hits(&uri, &target, true, false).into_iter().map(|h| h.span).collect(),
            Target::Syms { locs, name, .. } => {
                let scan = Scan { index: &self.index, targets: locs, name, include_decl: true, verify_text: false };
                let seed = self.view_variables(&uri);
                scan.run(&uri, &doc.text, Some(&doc.parse), &seed)
            }
        };
        let hl: Vec<DocumentHighlight> = spans
            .into_iter()
            .map(|sp| DocumentHighlight {
                range: range(&doc.text, &doc.lines, sp),
                kind: Some(DocumentHighlightKind::TEXT),
            })
            .collect();
        serde_json::to_value(hl).ok()
    }

    pub fn prepare_rename(&mut self, p: TextDocumentPositionParams) -> Option<Value> {
        let uri = canon(&p.text_document.uri);
        let offset = {
            let doc = self.docs.get(&uri)?;
            offset_of(&doc.text, &doc.lines, p.position)
        };
        let (target, origin) = self.target_at(&uri, offset)?;
        if let Target::Syms { locs, .. } = &target {
            if self.index.is_library(self.index.uri_of(*locs.first()?)) {
                self.show_message(
                    MessageType::WARNING,
                    "PHP LSP: symbols declared in vendor/ or built-in stubs cannot be renamed.",
                );
                return None;
            }
        }
        let doc = self.docs.get(&uri)?;
        let placeholder = origin.text(&doc.text).to_string();
        serde_json::to_value(PrepareRenameResponse::RangeWithPlaceholder {
            range: range(&doc.text, &doc.lines, origin),
            placeholder,
        })
        .ok()
    }

    pub fn rename(&mut self, p: RenameParams) -> Option<Value> {
        let uri = canon(&p.text_document_position.text_document.uri);
        let offset = {
            let doc = self.docs.get(&uri)?;
            offset_of(&doc.text, &doc.lines, p.text_document_position.position)
        };
        let new_name = p.new_name.trim_start_matches('$').to_string();
        let valid = !new_name.is_empty()
            && !new_name.as_bytes()[0].is_ascii_digit()
            && new_name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80);
        if !valid {
            self.show_message(MessageType::ERROR, &format!("PHP LSP: '{}' is not a valid PHP identifier.", p.new_name));
            return None;
        }
        let (target, _) = self.target_at(&uri, offset)?;
        if let Target::Syms { locs, .. } = &target {
            if self.index.is_library(self.index.uri_of(*locs.first()?)) {
                self.show_message(
                    MessageType::WARNING,
                    "PHP LSP: symbols declared in vendor/ or built-in stubs cannot be renamed.",
                );
                return None;
            }
        }
        let started = std::time::Instant::now();
        let hits = self.find_hits(&uri, &target, true, true);
        if hits.is_empty() {
            return None;
        }
        let mut changes: HashMap<Uri, Vec<TextEdit>> = HashMap::new();
        for h in &hits {
            changes.entry(lsp_uri(&h.uri)).or_default().push(TextEdit { range: h.range, new_text: new_name.clone() });
        }
        log::info!(
            "rename -> '{}': {} edit(s) in {} file(s) in {:.1} ms",
            new_name,
            hits.len(),
            changes.len(),
            started.elapsed().as_secs_f64() * 1000.0
        );

        // Classes: also rename the file when it follows the `ClassName.php` convention.
        let file_rename = match &target {
            Target::Syms { locs, name, kind } if kind.is_class_like() => {
                self.class_file_rename(locs[0], name, &new_name)
            }
            _ => None,
        };

        let edit = match file_rename {
            None => WorkspaceEdit { changes: Some(changes), ..Default::default() },
            Some((old, new)) => {
                let mut ops: Vec<DocumentChangeOperation> = changes
                    .into_iter()
                    .map(|(u, edits)| {
                        DocumentChangeOperation::Edit(TextDocumentEdit {
                            text_document: OptionalVersionedTextDocumentIdentifier { uri: u, version: None },
                            edits: edits.into_iter().map(OneOf::Left).collect(),
                        })
                    })
                    .collect();
                ops.push(DocumentChangeOperation::Op(ResourceOp::Rename(RenameFile {
                    old_uri: old,
                    new_uri: new,
                    options: None,
                    annotation_id: None,
                })));
                WorkspaceEdit { document_changes: Some(DocumentChanges::Operations(ops)), ..Default::default() }
            }
        };
        serde_json::to_value(edit).ok()
    }

    /// `(old, new)` file URI when the class at `decl` is the only class-like of `Name.php`.
    fn class_file_rename(&self, decl: SymLoc, name: &str, new_name: &str) -> Option<(Uri, Uri)> {
        let decl_uri = self.index.uri_of(decl).to_string();
        let path = uri_to_path(&decl_uri)?;
        let file = self.index.file_by_uri(&decl_uri)?;
        let class_like = file.symbols.iter().filter(|s| s.kind.is_class_like()).count();
        let stem_matches =
            path.file_stem().and_then(|s| s.to_str()).map(|s| s.eq_ignore_ascii_case(name)).unwrap_or(false);
        let new_path = path.with_file_name(format!("{new_name}.php"));
        let is_php = path.extension().map(|e| e == "php").unwrap_or(false);
        (class_like == 1 && stem_matches && is_php && !new_path.exists())
            .then(|| (lsp_uri(&decl_uri), lsp_uri(&path_to_uri(&new_path))))
    }
}
