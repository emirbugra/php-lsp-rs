//! Quick fixes and source actions.

use crate::convert::*;
use crate::server::Server;
use lsp_types::*;
use php_index::{unused_uses, ParamInfo, SymLoc, Symbol, SymbolKind, UnusedUse};
use php_syntax::{LineIndex, Span, Stmt, StmtKind, UseKind};
use serde_json::Value;
use std::collections::HashMap;

fn args_of(d: &Diagnostic) -> Vec<String> {
    d.data
        .as_ref()
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default()
}

fn kind_allowed(only: &Option<Vec<CodeActionKind>>, kind: &CodeActionKind) -> bool {
    match only {
        None => true,
        Some(o) => o.iter().any(|k| kind.as_str().starts_with(k.as_str())),
    }
}

/// Class declaration (span) whose name starts at `name_start`.
fn find_class_span(stmts: &[Stmt], name_start: u32) -> Option<Span> {
    for s in stmts {
        match &s.kind {
            StmtKind::Class(c) if c.name.as_ref().map(|n| n.span.start) == Some(name_start) => return Some(c.span),
            StmtKind::Namespace { body: Some(b), .. } => {
                if let Some(x) = find_class_span(b, name_start) {
                    return Some(x);
                }
            }
            StmtKind::Control { blocks, .. } => {
                for b in blocks {
                    if let Some(x) = find_class_span(b, name_start) {
                        return Some(x);
                    }
                }
            }
            StmtKind::Block(b) => {
                if let Some(x) = find_class_span(b, name_start) {
                    return Some(x);
                }
            }
            _ => {}
        }
    }
    None
}

fn is_builtin_type(w: &str) -> bool {
    matches!(
        w.to_ascii_lowercase().as_str(),
        "int"
            | "integer"
            | "string"
            | "bool"
            | "boolean"
            | "float"
            | "double"
            | "array"
            | "object"
            | "mixed"
            | "void"
            | "null"
            | "callable"
            | "iterable"
            | "resource"
            | "false"
            | "true"
            | "never"
            | "self"
            | "static"
            | "parent"
    )
}

/// Rewrite class names inside a type to fully qualified ones; None when the type is not valid PHP syntax.
fn qualify_type(ty: &str, classes: &[php_index::IStr]) -> Option<String> {
    if !ty
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'\\' | b'?' | b'|' | b'&' | b'(' | b')') || b >= 0x80)
    {
        return None;
    }
    let mut out = String::new();
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        if word.is_empty() {
            return;
        }
        if is_builtin_type(word) || word.starts_with('\\') {
            out.push_str(word);
        } else {
            let short = word.rsplit('\\').next().unwrap_or(word);
            match classes.iter().find(|c| c.rsplit('\\').next().map(|s| s.eq_ignore_ascii_case(short)).unwrap_or(false))
            {
                Some(c) => {
                    out.push('\\');
                    out.push_str(c);
                }
                None => out.push_str(word),
            }
        }
        word.clear();
    };
    for ch in ty.chars() {
        if ch.is_alphanumeric() || ch == '_' || ch == '\\' {
            word.push(ch);
        } else {
            flush(&mut word, &mut out);
            out.push(ch);
        }
    }
    flush(&mut word, &mut out);
    Some(out)
}

fn param_decl(p: &ParamInfo) -> String {
    let mut s = String::new();
    if let Some(t) = &p.ty {
        if let Some(q) = qualify_type(t, &p.classes) {
            s.push_str(&q);
            s.push(' ');
        }
    }
    if p.by_ref {
        s.push('&');
    }
    if p.variadic {
        s.push_str("...");
    }
    s.push('$');
    s.push_str(&p.name);
    if let Some(d) = &p.default {
        s.push_str(" = ");
        s.push_str(d);
    }
    s
}

fn method_stub(sym: &Symbol, indent: &str) -> String {
    let vis = match sym.vis {
        php_syntax::Visibility::Public => "public",
        php_syntax::Visibility::Protected => "protected",
        php_syntax::Visibility::Private => "private",
    };
    let params = sym.params.iter().map(param_decl).collect::<Vec<_>>().join(", ");
    let ret =
        sym.ty.as_ref().and_then(|t| qualify_type(t, &sym.type_classes)).map(|t| format!(": {t}")).unwrap_or_default();
    format!(
        "{indent}{vis}{} function {}({params}){ret}\n{indent}{{\n{indent}    // TODO: Implement {}() method.\n{indent}}}",
        if sym.is_static { " static" } else { "" },
        sym.name,
        sym.name
    )
}

/// Text edit inserting `stubs` just before the closing brace of the class spanning `class_span`.
fn insert_members(text: &str, lines: &LineIndex, class_span: Span, stubs: &[String]) -> Option<TextEdit> {
    let close = class_span.end as usize;
    if close == 0 || text.as_bytes().get(close - 1) != Some(&b'}') {
        return None;
    }
    let brace = close - 1;
    let line_start = text[..brace].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let before_brace = &text[line_start..brace];
    let at_line_start = before_brace.trim().is_empty();
    let before = text[..brace].trim_end();
    let after_open_brace = before.ends_with('{');
    let mut body = String::new();
    if !after_open_brace {
        body.push('\n');
    }
    body.push_str(&stubs.join("\n\n"));
    body.push('\n');
    let pos = if at_line_start { line_start } else { brace };
    if !at_line_start {
        body = format!("\n{body}");
    }
    let (l, c) = lines.position(text, pos);
    let p = Position { line: l, character: c };
    Some(TextEdit { range: Range { start: p, end: p }, new_text: body })
}

fn indent_of_class(text: &str, class_span: Span) -> String {
    let start = class_span.start as usize;
    let line_start = text[..start].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let lead: String = text[line_start..].chars().take_while(|c| *c == ' ' || *c == '\t').collect();
    format!("{lead}    ")
}

/// Delete range for removing one import.
fn removal_range(text: &str, u: &UnusedUse) -> Span {
    if u.whole_stmt {
        let mut start = u.stmt_span.start as usize;
        let mut end = u.stmt_span.end as usize;
        let ls = text[..start].rfind('\n').map(|i| i + 1).unwrap_or(0);
        if text[ls..start].trim().is_empty() {
            start = ls;
        }
        let rest = &text[end..];
        let spaces = rest.len() - rest.trim_start_matches([' ', '\t']).len();
        let rest2 = &rest[spaces..];
        if rest2.starts_with("\r\n") {
            end += spaces + 2;
        } else if rest2.starts_with('\n') {
            end += spaces + 1;
        }
        return Span::new(start, end);
    }
    let s = u.item_span.start as usize;
    let e = u.item_span.end as usize;
    let rest = &text[e..];
    let t = rest.trim_start();
    if let Some(after) = t.strip_prefix(',') {
        let skip = rest.len() - after.trim_start_matches([' ', '\t']).len();
        return Span::new(s, e + skip);
    }
    // last item: remove the comma before it
    let before = text[..s].trim_end();
    if before.ends_with(',') {
        return Span::new(before.len() - 1, e);
    }
    Span::new(s, e)
}

impl Server {
    pub fn code_actions(&mut self, p: CodeActionParams) -> Option<Value> {
        let uri = canon(&p.text_document.uri);
        let mut actions: Vec<CodeActionOrCommand> = Vec::new();
        let only = p.context.only.clone();

        let quick = kind_allowed(&only, &CodeActionKind::QUICKFIX);
        if quick {
            for d in &p.context.diagnostics {
                if d.source.as_deref() != Some("php-lsp") {
                    continue;
                }
                let Some(NumberOrString::String(code)) = &d.code else { continue };
                let args = args_of(d);
                match code.as_str() {
                    "undefined-class" => self.fix_import_class(&uri, d, &args, &mut actions),
                    "unused-import" => self.fix_unused_import(&uri, d, &mut actions),
                    "missing-abstract" => self.fix_missing_abstract(&uri, d, &args, &mut actions),
                    "undefined-method" => self.fix_create_method(&uri, d, &args, &mut actions),
                    c if c.starts_with("laravel-unknown-") => {
                        if let Some(name) = args.first() {
                            let mut changes = HashMap::new();
                            changes.insert(lsp_uri(&uri), vec![TextEdit { range: d.range, new_text: name.clone() }]);
                            actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                                title: format!("Change to '{name}'"),
                                kind: Some(CodeActionKind::QUICKFIX),
                                diagnostics: Some(vec![d.clone()]),
                                edit: Some(WorkspaceEdit { changes: Some(changes), ..Default::default() }),
                                is_preferred: Some(true),
                                ..Default::default()
                            }));
                        }
                    }
                    _ => {}
                }
            }
        }
        if kind_allowed(&only, &CodeActionKind::SOURCE_ORGANIZE_IMPORTS) {
            self.action_organize_imports(&uri, &mut actions);
        }
        if kind_allowed(&only, &CodeActionKind::REFACTOR_REWRITE) {
            self.type_actions(&uri, p.range, &mut actions);
        }
        self.refactor_actions(&uri, p.range, &only, &mut actions);
        if kind_allowed(&only, &CodeActionKind::new("refactor.move")) {
            self.move_class_action(&uri, p.range, &mut actions);
        }
        serde_json::to_value(actions).ok()
    }

    fn fix_import_class(&mut self, uri: &str, d: &Diagnostic, args: &[String], out: &mut Vec<CodeActionOrCommand>) {
        let Some(written) = args.get(1) else { return };
        if written.contains('\\') {
            return;
        }
        let Some(analysis) = self.analyze_doc(uri) else { return };
        let Some(doc) = self.docs.get(uri) else { return };
        let offset = offset_of(&doc.text, &doc.lines, d.range.start);
        let mut cands: Vec<(String, SymbolKind)> = self
            .index
            .all_classes()
            .map(|l| self.index.sym(l))
            .filter(|s| s.name.eq_ignore_ascii_case(written))
            .map(|s| (s.fqn.to_string(), s.kind))
            .collect();
        cands.sort_by(|a, b| a.0.cmp(&b.0));
        cands.dedup_by(|a, b| a.0 == b.0);
        // project classes before vendor ones
        cands.sort_by_key(|(f, _)| {
            self.index.find_class(f).map(|l| self.index.is_library(self.index.uri_of(l))).unwrap_or(true)
        });
        for (i, (fqn, kind)) in cands.into_iter().take(6).enumerate() {
            let Some(edit) = self.import_edit(uri, offset, &analysis, &fqn, kind) else { continue };
            let mut changes = HashMap::new();
            changes.insert(lsp_uri(uri), vec![edit]);
            out.push(CodeActionOrCommand::CodeAction(CodeAction {
                title: format!("Import class '{fqn}'"),
                kind: Some(CodeActionKind::QUICKFIX),
                diagnostics: Some(vec![d.clone()]),
                edit: Some(WorkspaceEdit { changes: Some(changes), ..Default::default() }),
                is_preferred: Some(i == 0),
                ..Default::default()
            }));
        }
    }

    fn fix_unused_import(&mut self, uri: &str, d: &Diagnostic, out: &mut Vec<CodeActionOrCommand>) {
        let Some(doc) = self.docs.get(uri) else { return };
        let start = offset_of(&doc.text, &doc.lines, d.range.start) as u32;
        let Some(u) = unused_uses(&doc.text, &doc.parse.stmts).into_iter().find(|u| u.item_span.start == start) else {
            return;
        };
        let r = removal_range(&doc.text, &u);
        let mut changes = HashMap::new();
        changes
            .insert(lsp_uri(uri), vec![TextEdit { range: range(&doc.text, &doc.lines, r), new_text: String::new() }]);
        out.push(CodeActionOrCommand::CodeAction(CodeAction {
            title: "Remove unused import".into(),
            kind: Some(CodeActionKind::QUICKFIX),
            diagnostics: Some(vec![d.clone()]),
            edit: Some(WorkspaceEdit { changes: Some(changes), ..Default::default() }),
            is_preferred: Some(true),
            ..Default::default()
        }));
    }

    fn missing_stubs(&self, fqn: &str, indent: &str) -> Vec<String> {
        self.index.missing_abstract(fqn).into_iter().map(|l: SymLoc| method_stub(self.index.sym(l), indent)).collect()
    }

    fn fix_missing_abstract(&mut self, uri: &str, d: &Diagnostic, args: &[String], out: &mut Vec<CodeActionOrCommand>) {
        let Some(fqn) = args.first() else { return };
        let Some(doc) = self.docs.get(uri) else { return };
        let name_start = offset_of(&doc.text, &doc.lines, d.range.start) as u32;
        let Some(cspan) = find_class_span(&doc.parse.stmts, name_start) else { return };
        let indent = indent_of_class(&doc.text, cspan);
        let stubs = self.missing_stubs(fqn, &indent);
        if stubs.is_empty() {
            return;
        }
        let Some(edit) = insert_members(&doc.text, &doc.lines, cspan, &stubs) else { return };
        let mut changes = HashMap::new();
        changes.insert(lsp_uri(uri), vec![edit]);
        out.push(CodeActionOrCommand::CodeAction(CodeAction {
            title: format!("Implement {} missing method(s)", stubs.len()),
            kind: Some(CodeActionKind::QUICKFIX),
            diagnostics: Some(vec![d.clone()]),
            edit: Some(WorkspaceEdit { changes: Some(changes), ..Default::default() }),
            is_preferred: Some(true),
            ..Default::default()
        }));
    }

    fn fix_create_method(&mut self, uri: &str, d: &Diagnostic, args: &[String], out: &mut Vec<CodeActionOrCommand>) {
        let (Some(name), Some(class_fqn)) = (args.first(), args.get(1)) else { return };
        let Some(doc) = self.docs.get(uri) else { return };
        let at = offset_of(&doc.text, &doc.lines, d.range.start) as u32;
        let Some(call) = php_index::inspections::find_call(&doc.parse.stmts, at) else { return };

        let Some(loc) = self.index.find_class(class_fqn) else { return };
        let target_uri = self.index.uri_of(loc).to_string();
        if self.index.is_library(&target_uri) {
            return;
        }
        let csym = self.index.sym(loc);
        let Some(target_text) = self.text_for(&target_uri) else { return };
        let target_text = target_text.into_owned();
        let target_parse = match self.docs.get(&target_uri) {
            Some(d) => (*d.parse).clone(),
            None => php_syntax::parse(&target_text),
        };
        let Some(cspan) = find_class_span(&target_parse.stmts, csym.name_span.start) else { return };
        let indent = indent_of_class(&target_text, cspan);
        let params = call.arg_names.iter().map(|n| format!("${n}")).collect::<Vec<_>>().join(", ");
        let vis = if call.on_self { "private" } else { "public" };
        let _ = vis;
        let stub = format!(
            "{indent}public{} function {name}({params})\n{indent}{{\n{indent}    // TODO: Implement {name}() method.\n{indent}}}",
            if call.is_static { " static" } else { "" },
        );
        let lines = LineIndex::new(&target_text);
        let Some(edit) = insert_members(&target_text, &lines, cspan, &[stub]) else { return };
        let mut changes = HashMap::new();
        changes.insert(lsp_uri(&target_uri), vec![edit]);
        let short = csym.name;
        out.push(CodeActionOrCommand::CodeAction(CodeAction {
            title: if target_uri == uri {
                format!("Create method '{name}'")
            } else {
                format!("Create method '{name}' in {short}")
            },
            kind: Some(CodeActionKind::QUICKFIX),
            diagnostics: Some(vec![d.clone()]),
            edit: Some(WorkspaceEdit { changes: Some(changes), ..Default::default() }),
            ..Default::default()
        }));
    }

    /// Remove unused imports and sort the remaining ones (when they form one simple block).
    fn action_organize_imports(&mut self, uri: &str, out: &mut Vec<CodeActionOrCommand>) {
        let Some(doc) = self.docs.get(uri) else { return };
        let unused = unused_uses(&doc.text, &doc.parse.stmts);

        // collect top-level use statements of the first namespace level
        fn level_uses(stmts: &[Stmt]) -> Vec<&Stmt> {
            let mut v = Vec::new();
            for s in stmts {
                match &s.kind {
                    StmtKind::Use(_) => v.push(s),
                    StmtKind::Namespace { body: Some(b), .. } => {
                        let inner = level_uses(b);
                        if !inner.is_empty() {
                            return inner;
                        }
                    }
                    _ => {}
                }
            }
            v
        }
        let uses = level_uses(&doc.parse.stmts);
        if uses.is_empty() {
            return;
        }

        let simple = uses.iter().all(|s| matches!(&s.kind, StmtKind::Use(i) if i.len() == 1 && !doc.text[s.span.start as usize..s.span.end as usize].contains('{')));
        let contiguous =
            uses.windows(2).all(|w| doc.text[w[0].span.end as usize..w[1].span.start as usize].trim().is_empty());

        let edits: Vec<TextEdit> = if simple && contiguous {
            let mut items: Vec<(u8, String, String)> = Vec::new(); // (kind order, sort key, rendered)
            for s in &uses {
                let StmtKind::Use(its) = &s.kind else { continue };
                let it = &its[0];
                if unused.iter().any(|u| u.stmt_span == s.span) {
                    continue;
                }
                let (order, kw) = match it.kind {
                    UseKind::Class => (0, "use"),
                    UseKind::Function => (1, "use function"),
                    UseKind::Const => (2, "use const"),
                };
                let alias = it.alias.as_ref().map(|a| format!(" as {a}")).unwrap_or_default();
                items.push((order, it.name.to_ascii_lowercase(), format!("{kw} {}{alias};", it.name)));
            }
            items.sort();
            items.dedup();
            let first = uses[0].span.start as usize;
            let last = uses[uses.len() - 1].span.end as usize;
            let ls = doc.text[..first].rfind('\n').map(|i| i + 1).unwrap_or(0);
            let indent: String = doc.text[ls..first].chars().take_while(|c| c.is_whitespace()).collect();
            let block = items.into_iter().map(|(_, _, r)| r).collect::<Vec<_>>().join(&format!("\n{indent}"));
            let current = &doc.text[first..last];
            if current == block {
                return;
            }
            vec![TextEdit { range: range(&doc.text, &doc.lines, Span::new(first, last)), new_text: block }]
        } else {
            let mut spans: Vec<Span> = unused.iter().map(|u| removal_range(&doc.text, u)).collect();
            spans.sort_by_key(|s| s.start);
            spans.dedup();
            if spans.is_empty() {
                return;
            }
            spans
                .into_iter()
                .map(|s| TextEdit { range: range(&doc.text, &doc.lines, s), new_text: String::new() })
                .collect()
        };

        let mut changes = HashMap::new();
        changes.insert(lsp_uri(uri), edits);
        out.push(CodeActionOrCommand::CodeAction(CodeAction {
            title: "Optimize imports".into(),
            kind: Some(CodeActionKind::SOURCE_ORGANIZE_IMPORTS),
            edit: Some(WorkspaceEdit { changes: Some(changes), ..Default::default() }),
            ..Default::default()
        }));
    }
}
