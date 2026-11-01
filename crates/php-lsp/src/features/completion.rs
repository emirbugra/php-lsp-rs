use crate::convert::*;
use crate::features::completion_kind;
use crate::server::Server;
use lsp_types::*;
use php_index::{fuzzy_score, Analysis, RefTarget, SymLoc, Symbol, SymbolKind};
use php_syntax::StmtKind;
use serde_json::Value;

const MAX_ITEMS: usize = 150;
const PLACEHOLDER: &str = "__php_lsp_c__";

const KEYWORDS: &[&str] = &[
    "abstract",
    "and",
    "array",
    "as",
    "break",
    "callable",
    "case",
    "catch",
    "class",
    "clone",
    "const",
    "continue",
    "declare",
    "default",
    "do",
    "echo",
    "else",
    "elseif",
    "empty",
    "enddeclare",
    "endfor",
    "endforeach",
    "endif",
    "endswitch",
    "endwhile",
    "enum",
    "extends",
    "final",
    "finally",
    "fn",
    "for",
    "foreach",
    "function",
    "global",
    "if",
    "implements",
    "include",
    "include_once",
    "instanceof",
    "insteadof",
    "interface",
    "isset",
    "list",
    "match",
    "namespace",
    "new",
    "or",
    "print",
    "private",
    "protected",
    "public",
    "readonly",
    "require",
    "require_once",
    "return",
    "static",
    "switch",
    "throw",
    "trait",
    "try",
    "unset",
    "use",
    "var",
    "while",
    "xor",
    "yield",
];

#[derive(Debug)]
enum Ctx {
    /// `$a->` / `Foo::`
    Member {
        arrow: bool,
        prefix: String,
        start: usize,
        end: usize,
        static_prop_dollar: bool,
    },
    Variable {
        prefix: String,
        start: usize,
        end: usize,
    },
    Class {
        prefix: String,
        start: usize,
        end: usize,
        instantiate: bool,
        in_use: bool,
    },
    Generic {
        prefix: String,
        start: usize,
        end: usize,
    },
}

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'\\' || b >= 0x80
}

fn context(text: &str, offset: usize) -> Ctx {
    let b = text.as_bytes();
    let offset = offset.min(b.len());
    let mut start = offset;
    while start > 0 && is_word(b[start - 1]) {
        start -= 1;
    }
    let mut end = offset;
    while end < b.len() && is_word(b[end]) {
        end += 1;
    }
    let prefix = text[start..offset].to_string();

    // look at what precedes the word, skipping whitespace
    let mut p = start;
    while p > 0 && b[p - 1].is_ascii_whitespace() {
        p -= 1;
    }
    let ends = |s: &str| text[..p].ends_with(s);

    if start > 0 && b[start - 1] == b'$' {
        // `Foo::$bar` or plain `$var`
        let before = &text[..start - 1];
        if before.trim_end().ends_with("::") && before.ends_with("::") {
            return Ctx::Member { arrow: false, prefix, start, end, static_prop_dollar: true };
        }
        return Ctx::Variable { prefix, start, end };
    }
    if ends("->") || ends("?->") {
        return Ctx::Member { arrow: true, prefix, start, end, static_prop_dollar: false };
    }
    if ends("::") && p == start {
        return Ctx::Member { arrow: false, prefix, start, end, static_prop_dollar: false };
    }
    // previous word
    let mut q = p;
    while q > 0 && is_word(b[q - 1]) {
        q -= 1;
    }
    let prev_word = text[q..p].to_ascii_lowercase();
    match prev_word.as_str() {
        "new" => return Ctx::Class { prefix, start, end, instantiate: true, in_use: false },
        "use" => {
            // top-level `use` (not closure `use (` — that would end with a paren)
            let line_start = text[..q].rfind('\n').map(|i| i + 1).unwrap_or(0);
            if text[line_start..q].trim().is_empty() || text[line_start..q].trim() == "<?php" {
                return Ctx::Class { prefix, start, end, instantiate: false, in_use: true };
            }
        }
        "extends" | "implements" | "instanceof" | "insteadof" => {
            return Ctx::Class { prefix, start, end, instantiate: false, in_use: false }
        }
        _ => {}
    }
    if ends("catch (") || ends("catch(") || ends("|") && text[..p].contains("catch") {
        return Ctx::Class { prefix, start, end, instantiate: false, in_use: false };
    }
    Ctx::Generic { prefix, start, end }
}

fn short_ns(fqn: &str) -> &str {
    fqn.rsplit_once('\\').map(|(ns, _)| ns).unwrap_or("")
}

impl Server {
    pub fn completion(&mut self, p: CompletionParams) -> Option<Value> {
        let pos = p.text_document_position;
        let uri = canon(&pos.text_document.uri);
        let offset = {
            let doc = self.docs.get(&uri)?;
            offset_of(&doc.text, &doc.lines, pos.position)
        };
        if let Some(items) = self.laravel_completion(&uri, offset) {
            return serde_json::to_value(CompletionResponse::List(CompletionList { is_incomplete: false, items })).ok();
        }
        let doc = self.docs.get(&uri)?;
        // quotes and `.` only trigger completion for Laravel strings; inside other plain strings
        // (outside class names) there is nothing useful to offer
        let trigger = p.context.as_ref().and_then(|c| c.trigger_character.as_deref());
        let in_string = super::laravel::string_at(&doc.text, offset)
            .map(|l| !doc.text[l.start..offset].contains('\\'))
            .unwrap_or(false);
        if matches!(trigger, Some("'") | Some("\"") | Some(".")) || in_string {
            return serde_json::to_value(CompletionResponse::List(CompletionList {
                is_incomplete: false,
                items: Vec::new(),
            }))
            .ok();
        }
        let ctx = context(&doc.text, offset);
        let (items, incomplete) = match ctx {
            Ctx::Member { arrow, prefix, start, end, static_prop_dollar } => {
                (self.member_items(&uri, arrow, &prefix, start..end, static_prop_dollar), false)
            }
            Ctx::Variable { prefix, start, end } => (self.variable_items(&uri, offset, &prefix, start, end), false),
            Ctx::Class { prefix, start, end, instantiate, in_use } => {
                self.class_items(&uri, &prefix, start, end, instantiate, in_use)
            }
            Ctx::Generic { prefix, start, end } => self.generic_items(&uri, &prefix, start, end),
        };
        serde_json::to_value(CompletionResponse::List(CompletionList { is_incomplete: incomplete, items })).ok()
    }

    pub(crate) fn doc_range(&self, uri: &str, start: usize, end: usize) -> Option<Range> {
        let doc = self.docs.get(uri)?;
        Some(Range {
            start: {
                let (l, c) = doc.lines.position(&doc.text, start);
                Position { line: l, character: c }
            },
            end: {
                let (l, c) = doc.lines.position(&doc.text, end);
                Position { line: l, character: c }
            },
        })
    }

    fn visible(&self, sym: &Symbol, cur_class: Option<&str>) -> bool {
        match sym.vis {
            php_syntax::Visibility::Public => true,
            php_syntax::Visibility::Protected => match (cur_class, &sym.container) {
                (Some(cur), Some(owner)) => {
                    cur.eq_ignore_ascii_case(owner)
                        || self.index.is_subclass_of(cur, owner)
                        || self.index.is_subclass_of(owner, cur)
                }
                _ => false,
            },
            php_syntax::Visibility::Private => match (cur_class, &sym.container) {
                (Some(cur), Some(owner)) => cur.eq_ignore_ascii_case(owner),
                _ => false,
            },
        }
    }

    /// Members after `->` / `::`; `word` is the partially typed member name.
    fn member_items(
        &mut self,
        uri: &str,
        arrow: bool,
        prefix: &str,
        word: std::ops::Range<usize>,
        dollar: bool,
    ) -> Vec<CompletionItem> {
        let Some((classes, cur_class)) = self.receiver_classes(uri, word.clone()) else { return Vec::new() };
        let (start, end) = (word.start, word.end);
        let Some(range) = self.doc_range(uri, start, end) else { return Vec::new() };
        let next_is_paren = self.docs.get(uri).map(|d| d.text[end..].starts_with('(')).unwrap_or(false);
        let filter_prefix = prefix.trim_start_matches('$');
        let mut items = Vec::new();
        let mut seen = rustc_hash::FxHashSet::default();
        for (class, only_scopes) in self.member_sources(&classes) {
            let from_builder = {
                let b = php_index::strip_generic(&class);
                b.eq_ignore_ascii_case(php_index::index::ELOQUENT_BUILDER)
                    || b.eq_ignore_ascii_case(php_index::index::QUERY_BUILDER)
            };
            for loc in self.index.all_members(&class) {
                let s = self.index.sym(loc);
                if only_scopes && !(s.is_virtual && s.kind == SymbolKind::Method) {
                    continue;
                }
                if from_builder && (s.kind != SymbolKind::Method || s.vis != php_syntax::Visibility::Public) {
                    continue;
                }
                if !self.visible(s, cur_class.as_deref()) {
                    continue;
                }
                let ok = match s.kind {
                    SymbolKind::Method => true,
                    // `$obj->prop` for instance properties, `Cls::$prop` for static ones
                    SymbolKind::Property => arrow != s.is_static,
                    SymbolKind::ClassConst | SymbolKind::EnumCase => !arrow,
                    _ => false,
                };
                if !ok {
                    continue;
                }
                if s.name.starts_with("__") && !filter_prefix.starts_with('_') {
                    continue;
                }
                let Some(score) = fuzzy_score(filter_prefix, &s.name) else { continue };
                if !seen.insert((s.kind, s.name.to_ascii_lowercase())) {
                    continue;
                }
                items.push(self.member_item(loc, score, range, next_is_paren, !arrow && !dollar));
            }
        }
        items.sort_by(|a, b| a.sort_text.cmp(&b.sort_text));
        items.truncate(MAX_ITEMS);
        items
    }

    /// Types of the receiver before `->` / `::` and the class the cursor is in. The partial word is
    /// replaced by a placeholder so the parser sees a complete member access.
    fn receiver_classes(&mut self, uri: &str, word: std::ops::Range<usize>) -> Option<(Vec<String>, Option<String>)> {
        let doc = self.docs.get(uri)?;
        let mut patched = String::with_capacity(doc.text.len() + PLACEHOLDER.len());
        patched.push_str(&doc.text[..word.start]);
        patched.push_str(PLACEHOLDER);
        patched.push_str(&doc.text[word.end..]);
        let parse = php_syntax::parse(&patched);
        let analysis = php_index::analyze(&self.index, &patched, &parse.stmts);
        let mut classes = analysis.refs.iter().find_map(|r| match &r.target {
            RefTarget::Member { name, classes, .. } if name == PLACEHOLDER => Some(classes.clone()),
            _ => None,
        })?;
        let cur_class = analysis.class_at(word.start).map(|c| c.fqn.clone());
        // classes that are not indexed yet (composer autoload)
        for c in classes.clone() {
            self.autoload(&c);
        }
        classes.dedup();
        Some((classes, cur_class))
    }

    /// Classes whose members are offered, with "only scopes" flags. Eloquent models forward to the
    /// query builders; builders expose the scopes of their model.
    fn member_sources(&self, classes: &[String]) -> Vec<(String, bool)> {
        let mut sources = Vec::new();
        for class in classes {
            sources.push((class.clone(), false));
            let base = php_index::strip_generic(class);
            if base.eq_ignore_ascii_case(php_index::index::ELOQUENT_BUILDER) {
                sources.push((php_index::index::QUERY_BUILDER.to_string(), false));
                if let Some(m) = php_index::generic_arg(class) {
                    sources.push((m.to_string(), true));
                }
            } else if self.index.is_model(base) {
                sources.push((php_index::index::ELOQUENT_BUILDER.to_string(), false));
                sources.push((php_index::index::QUERY_BUILDER.to_string(), false));
            }
        }
        sources
    }

    fn member_item(
        &self,
        loc: SymLoc,
        score: u32,
        range: Range,
        next_is_paren: bool,
        static_dollar: bool,
    ) -> CompletionItem {
        let s = self.index.sym(loc);
        let (text, fmt) = match s.kind {
            SymbolKind::Method if !next_is_paren => {
                if s.params.is_empty() {
                    (format!("{}()$0", s.name), InsertTextFormat::SNIPPET)
                } else {
                    (format!("{}($1)$0", s.name), InsertTextFormat::SNIPPET)
                }
            }
            SymbolKind::Property if static_dollar => (format!("${}", s.name), InsertTextFormat::PLAIN_TEXT),
            _ => (s.name.to_string(), InsertTextFormat::PLAIN_TEXT),
        };
        let label =
            if s.kind == SymbolKind::Property && static_dollar { format!("${}", s.name) } else { s.name.to_string() };
        CompletionItem {
            label,
            kind: Some(completion_kind(s.kind)),
            detail: Some(s.detail.to_string()),
            tags: if s.deprecated { Some(vec![CompletionItemTag::DEPRECATED]) } else { None },
            sort_text: Some(format!(
                "{}{}{}",
                score,
                if s.is_virtual { "1" } else { "0" },
                s.name.to_ascii_lowercase()
            )),
            filter_text: Some(s.name.to_string()),
            insert_text_format: Some(fmt),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit { range, new_text: text })),
            ..Default::default()
        }
    }

    fn variable_items(&self, uri: &str, offset: usize, prefix: &str, start: usize, end: usize) -> Vec<CompletionItem> {
        let Some(analysis) = self.analyze_doc(uri) else { return Vec::new() };
        let Some(range) = self.doc_range(uri, start.saturating_sub(1), end) else { return Vec::new() };
        let mut items = Vec::new();
        let mut names: Vec<(String, Vec<String>)> =
            analysis.vars_at(offset).map(|s| s.vars.clone()).unwrap_or_default();
        if analysis.class_at(offset).is_some() {
            let cls = analysis.class_at(offset).map(|c| vec![c.fqn.clone()]).unwrap_or_default();
            names.push(("this".to_string(), cls));
        }
        for g in ["_GET", "_POST", "_SERVER", "_SESSION", "_COOKIE", "_FILES", "_REQUEST", "_ENV", "GLOBALS"] {
            names.push((g.to_string(), Vec::new()));
        }
        for (name, classes) in names {
            let Some(score) = fuzzy_score(prefix, &name) else { continue };
            if name == prefix {
                continue; // the variable being typed itself
            }
            items.push(CompletionItem {
                label: format!("${name}"),
                kind: Some(CompletionItemKind::VARIABLE),
                detail: if classes.is_empty() { None } else { Some(classes.join("|")) },
                sort_text: Some(format!("{score}{name}")),
                filter_text: Some(format!("${name}")),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit { range, new_text: format!("${name}") })),
                ..Default::default()
            });
        }
        items
    }

    /// Where to insert a `use` statement and whether a blank line must precede it.
    fn use_insert(&self, uri: &str, offset: usize) -> Option<(Position, bool)> {
        let doc = self.docs.get(uri)?;
        let mut last_use_end: Option<usize> = None;
        let mut ns_end: Option<usize> = None;
        fn scan(
            stmts: &[php_syntax::Stmt],
            text: &str,
            offset: usize,
            last_use_end: &mut Option<usize>,
            ns_end: &mut Option<usize>,
        ) {
            for s in stmts {
                match &s.kind {
                    StmtKind::Use(_) => *last_use_end = Some(s.span.end as usize),
                    StmtKind::Namespace { body: None, .. } => {
                        *ns_end = Some(s.span.end as usize);
                        *last_use_end = None;
                    }
                    StmtKind::Namespace { body: Some(b), .. } if s.span.contains(offset) => {
                        let open = text[s.span.start as usize..].find('{').map(|i| s.span.start as usize + i + 1);
                        *ns_end = open;
                        *last_use_end = None;
                        scan(b, text, offset, last_use_end, ns_end);
                    }
                    _ => {}
                }
            }
        }
        scan(&doc.parse.stmts, &doc.text, offset, &mut last_use_end, &mut ns_end);
        let (pos, blank) = if let Some(e) = last_use_end {
            (e, false)
        } else if let Some(e) = ns_end {
            (e, true)
        } else {
            // after the opening tag
            let tag_end = doc
                .text
                .find("<?")
                .map(|i| doc.text[i..].find('\n').map(|n| i + n).unwrap_or(doc.text.len()))
                .unwrap_or(0);
            (tag_end, true)
        };
        let line_end = doc.text[pos..].find('\n').map(|n| pos + n).unwrap_or(doc.text.len());
        let (l, c) = doc.lines.position(&doc.text, line_end);
        Some((Position { line: l, character: c }, blank))
    }

    pub(crate) fn import_edit(
        &self,
        uri: &str,
        offset: usize,
        analysis: &Analysis,
        fqn: &str,
        kind: SymbolKind,
    ) -> Option<TextEdit> {
        let imported = match kind {
            SymbolKind::Function => analysis.ctx.functions.values().any(|v| v.eq_ignore_ascii_case(fqn)),
            SymbolKind::Const => analysis.ctx.consts.values().any(|v| v == fqn),
            _ => analysis.ctx.classes.values().any(|v| v.eq_ignore_ascii_case(fqn)),
        };
        if imported {
            return None;
        }
        let ns = &analysis.ctx.namespace;
        let sym_ns = short_ns(fqn);
        if sym_ns.eq_ignore_ascii_case(ns) {
            return None;
        }
        if sym_ns.is_empty() && ns.is_empty() {
            return None;
        }
        let (pos, blank) = self.use_insert(uri, offset)?;
        let kw = match kind {
            SymbolKind::Function => "use function ",
            SymbolKind::Const => "use const ",
            _ => "use ",
        };
        Some(TextEdit {
            range: Range { start: pos, end: pos },
            new_text: format!("{}\n{kw}{fqn};", if blank { "\n" } else { "" }),
        })
    }

    fn class_items(
        &self,
        uri: &str,
        prefix: &str,
        start: usize,
        end: usize,
        instantiate: bool,
        in_use: bool,
    ) -> (Vec<CompletionItem>, bool) {
        let Some(analysis) = self.analyze_doc(uri) else { return (Vec::new(), false) };
        let Some(range) = self.doc_range(uri, start, end) else { return (Vec::new(), false) };
        let absolute = prefix.starts_with('\\');
        let qualified = prefix.contains('\\');
        let pfx = prefix.trim_start_matches('\\');

        let mut hits: Vec<(u32, SymLoc)> = Vec::new();
        for loc in self.index.all_classes() {
            let s = self.index.sym(loc);
            if instantiate
                && (s.is_abstract || matches!(s.kind, SymbolKind::Interface | SymbolKind::Trait | SymbolKind::Enum))
            {
                continue;
            }
            let score = if qualified {
                if s.fqn.len() >= pfx.len() && s.fqn.as_bytes()[..pfx.len()].eq_ignore_ascii_case(pfx.as_bytes()) {
                    Some(1)
                } else {
                    None
                }
            } else {
                fuzzy_score(pfx, &s.name)
            };
            if let Some(sc) = score {
                hits.push((sc, loc));
            }
        }
        hits.sort_by(|a, b| {
            let (sa, sb) = (self.index.sym(a.1), self.index.sym(b.1));
            a.0.cmp(&b.0).then(sa.name.len().cmp(&sb.name.len())).then_with(|| sa.fqn.cmp(&sb.fqn))
        });
        let incomplete = hits.len() > MAX_ITEMS;
        hits.truncate(MAX_ITEMS);

        let offset = start;
        let items = hits
            .into_iter()
            .enumerate()
            .map(|(i, (_, loc))| {
                let s = self.index.sym(loc);
                let (text, edit) = if in_use || qualified {
                    (if absolute && !in_use { format!("\\{}", s.fqn) } else { s.fqn.to_string() }, None)
                } else {
                    (s.name.to_string(), self.import_edit(uri, offset, &analysis, &s.fqn, s.kind))
                };
                CompletionItem {
                    label: s.name.to_string(),
                    kind: Some(completion_kind(s.kind)),
                    detail: Some(s.fqn.to_string()),
                    tags: if s.deprecated { Some(vec![CompletionItemTag::DEPRECATED]) } else { None },
                    sort_text: Some(format!("{i:04}")),
                    filter_text: Some(if qualified { s.fqn.to_string() } else { s.name.to_string() }),
                    text_edit: Some(CompletionTextEdit::Edit(TextEdit { range, new_text: text })),
                    additional_text_edits: edit.map(|e| vec![e]),
                    ..Default::default()
                }
            })
            .collect();
        (items, incomplete)
    }

    fn generic_items(&self, uri: &str, prefix: &str, start: usize, end: usize) -> (Vec<CompletionItem>, bool) {
        if prefix.is_empty() {
            return (Vec::new(), false);
        }
        let (mut items, mut incomplete) = self.class_items(uri, prefix, start, end, false, false);
        let Some(analysis) = self.analyze_doc(uri) else { return (items, incomplete) };
        let Some(range) = self.doc_range(uri, start, end) else { return (items, incomplete) };

        let mut funcs: Vec<(u32, SymLoc)> = self
            .index
            .all_functions()
            .chain(self.index.all_consts())
            .filter_map(|l| fuzzy_score(prefix, &self.index.sym(l).name).map(|s| (s, l)))
            .collect();
        funcs.sort_by(|a, b| a.0.cmp(&b.0).then(self.index.sym(a.1).name.len().cmp(&self.index.sym(b.1).name.len())));
        if funcs.len() > 80 {
            incomplete = true;
            funcs.truncate(80);
        }
        for (i, (score, loc)) in funcs.into_iter().enumerate() {
            let s = self.index.sym(loc);
            let (text, fmt) = if s.kind == SymbolKind::Function {
                if s.params.is_empty() {
                    (format!("{}()$0", s.name), InsertTextFormat::SNIPPET)
                } else {
                    (format!("{}($1)$0", s.name), InsertTextFormat::SNIPPET)
                }
            } else {
                (s.name.to_string(), InsertTextFormat::PLAIN_TEXT)
            };
            items.push(CompletionItem {
                label: s.name.to_string(),
                kind: Some(completion_kind(s.kind)),
                detail: Some(s.detail.to_string()),
                tags: if s.deprecated { Some(vec![CompletionItemTag::DEPRECATED]) } else { None },
                sort_text: Some(format!("{}{i:04}", score + 1)),
                insert_text_format: Some(fmt),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit { range, new_text: text })),
                additional_text_edits: self.import_edit(uri, start, &analysis, &s.fqn, s.kind).map(|e| vec![e]),
                ..Default::default()
            });
        }
        for kw in KEYWORDS {
            if let Some(score) = fuzzy_score(prefix, kw) {
                items.push(CompletionItem {
                    label: (*kw).to_string(),
                    kind: Some(CompletionItemKind::KEYWORD),
                    sort_text: Some(format!("{}{kw}", score + 1)),
                    text_edit: Some(CompletionTextEdit::Edit(TextEdit { range, new_text: (*kw).to_string() })),
                    ..Default::default()
                });
            }
        }
        (items, incomplete)
    }
}
