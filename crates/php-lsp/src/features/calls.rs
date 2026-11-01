//! Call hierarchy: who calls a function / method (incoming) and what it calls (outgoing).
//!
//! Incoming calls reuse the find-references machinery and attribute every hit to the innermost
//! function / method declared around it. Outgoing calls analyse the body of the item.

use super::references::{ident_span, Target};
use crate::convert::*;
use crate::server::Server;
use crate::texts::Texts;
use lsp_types::*;
use php_index::{MemberKind, RefTarget, SymLoc, SymbolKind};
use php_syntax::{LineIndex, Span};
use serde_json::{json, Value};
use std::collections::HashMap;

fn is_callable(k: SymbolKind) -> bool {
    matches!(k, SymbolKind::Function | SymbolKind::Method)
}

/// `new` right before `span` (constructor call)?
fn preceded_by_new(text: &str, span: Span) -> bool {
    let before = text[..span.start as usize].trim_end();
    before.len() >= 3 && before[before.len() - 3..].eq_ignore_ascii_case("new")
}

/// Calls from top-level code: the file itself is the caller.
fn top_level_item(uri: &str, first: Range) -> CallHierarchyItem {
    CallHierarchyItem {
        name: uri.rsplit('/').next().unwrap_or(uri).to_string(),
        kind: lsp_types::SymbolKind::FILE,
        tags: None,
        detail: Some("(top level)".into()),
        uri: lsp_uri(uri),
        range: Range { start: Position { line: 0, character: 0 }, end: first.end },
        selection_range: first,
        data: None,
    }
}

impl Server {
    fn call_item(&self, texts: &mut Texts, loc: SymLoc) -> Option<CallHierarchyItem> {
        let sym = self.index.sym(loc).clone();
        let uri = self.index.uri_of(loc).to_string();
        let (text, lines) = texts.get(self, &uri)?;
        let mut sel = sym.name_span;
        if !sym.range.contains(sel.start as usize) {
            sel = Span { start: sym.range.start, end: sym.range.start };
        }
        Some(CallHierarchyItem {
            name: sym.name.to_string(),
            kind: super::lsp_symbol_kind(sym.kind, &sym.name),
            tags: sym.deprecated.then(|| vec![SymbolTag::DEPRECATED]),
            detail: Some(match &sym.container {
                Some(c) => c.trim_start_matches('\\').to_string(),
                None => sym.fqn.trim_start_matches('\\').to_string(),
            }),
            uri: lsp_uri(&uri),
            range: range(text, lines, sym.range),
            selection_range: range(text, lines, sel),
            data: Some(json!({
                "kind": if sym.kind == SymbolKind::Method { "method" } else { "function" },
                "fqn": sym.fqn.to_string(),
                "container": sym.container.as_ref().map(|c| c.to_string()),
                "name": sym.name.to_string(),
            })),
        })
    }

    /// Re-resolve an item handed back by the client (symbol indices are not stable across edits).
    fn item_loc(&self, item: &CallHierarchyItem) -> Option<SymLoc> {
        let d = item.data.as_ref()?;
        let name = d.get("name")?.as_str()?;
        match d.get("kind")?.as_str()? {
            "method" => self.index.find_member(d.get("container")?.as_str()?, name, MemberKind::Method),
            _ => self.index.find_function(d.get("fqn")?.as_str()?),
        }
    }

    pub fn prepare_call_hierarchy(&mut self, p: CallHierarchyPrepareParams) -> Option<Value> {
        let pos = p.text_document_position_params;
        let uri = canon(&pos.text_document.uri);
        let offset = {
            let doc = self.docs.get(&uri)?;
            offset_of(&doc.text, &doc.lines, pos.position)
        };
        let Target::Syms { locs, .. } = self.target_at(&uri, offset)?.0 else { return None };
        let mut texts = Texts::default();
        let mut items = Vec::new();
        for l in locs {
            let s = self.index.sym(l);
            let loc = if is_callable(s.kind) {
                Some(l)
            } else if s.kind.is_class_like() {
                // `new Foo` / class name: its constructor
                let fqn = s.fqn.to_string();
                self.index.find_member(&fqn, "__construct", MemberKind::Method)
            } else {
                None
            };
            if let Some(item) = loc.and_then(|l| self.call_item(&mut texts, l)) {
                items.push(item);
            }
        }
        if items.is_empty() {
            return None;
        }
        serde_json::to_value(items).ok()
    }

    pub fn incoming_calls(&mut self, p: CallHierarchyIncomingCallsParams) -> Option<Value> {
        let loc = self.item_loc(&p.item)?;
        let sym = self.index.sym(loc).clone();
        // constructors are called through `new ClassName`
        let ctor = sym.kind == SymbolKind::Method && sym.name.eq_ignore_ascii_case("__construct");
        let target = if ctor {
            let class = self.index.find_class(sym.container.as_deref()?)?;
            let cs = self.index.sym(class);
            Target::Syms { locs: vec![class], name: cs.name.to_string(), kind: cs.kind }
        } else {
            Target::Syms { locs: self.with_overridden(loc, &sym), name: sym.name.to_string(), kind: sym.kind }
        };
        let hits = self.find_hits("", &target, false, false);

        let mut texts = Texts::default();
        // caller (Some(symbol) or None = top-level code of a file) -> ranges
        let mut order: Vec<(String, Option<SymLoc>)> = Vec::new();
        let mut groups: HashMap<(String, Option<SymLoc>), Vec<Range>> = HashMap::new();
        for h in hits {
            if ctor {
                let Some((text, _)) = texts.get(self, &h.uri) else { continue };
                if !preceded_by_new(text, h.span) {
                    continue;
                }
            }
            let key = (h.uri.clone(), self.caller_at(&h.uri, h.span));
            groups.entry(key.clone()).or_insert_with(|| {
                order.push(key.clone());
                Vec::new()
            });
            groups.get_mut(&key).unwrap().push(h.range);
        }

        let mut out = Vec::new();
        for key in order {
            let ranges = groups.remove(&key).unwrap_or_default();
            let from = match key.1 {
                Some(l) => self.call_item(&mut texts, l),
                None => Some(top_level_item(&key.0, ranges[0])),
            };
            if let Some(from) = from {
                out.push(CallHierarchyIncomingCall { from, from_ranges: ranges });
            }
        }
        serde_json::to_value(out).ok()
    }

    /// The method at `loc` plus the interface / parent methods it overrides: calls through them
    /// may reach it too.
    fn with_overridden(&self, loc: SymLoc, sym: &php_index::Symbol) -> Vec<SymLoc> {
        let mut locs = vec![loc];
        if let Some(c) = sym.container.as_deref() {
            for a in self.index.ancestors(c) {
                // a trait method is shared by every class using the trait: its calls are not ours
                if self.index.sym(a).kind == SymbolKind::Trait {
                    continue;
                }
                if let Some(m) = self.direct_member(a, sym) {
                    if !locs.contains(&m) {
                        locs.push(m);
                    }
                }
            }
        }
        locs
    }

    /// Innermost function / method of `uri` around `span` (`None`: top-level code).
    fn caller_at(&self, uri: &str, span: Span) -> Option<SymLoc> {
        let fid = self.index.file_id(uri)?;
        let file = self.index.file_by_uri(uri)?;
        file.symbols
            .iter()
            .enumerate()
            .filter(|(_, s)| is_callable(s.kind) && !s.is_virtual && s.range.contains(span.start as usize))
            .min_by_key(|(_, s)| s.range.len())
            .map(|(i, _)| SymLoc { file: fid, idx: i as u32 })
    }

    pub fn outgoing_calls(&mut self, p: CallHierarchyOutgoingCallsParams) -> Option<Value> {
        let loc = self.item_loc(&p.item)?;
        let sym = self.index.sym(loc).clone();
        let uri = self.index.uri_of(loc).to_string();
        let mut texts = Texts::default();
        let text = texts.get(self, &uri)?.0.clone();
        let lines = LineIndex::new(&text);
        let parse = match self.docs.get(&uri) {
            Some(d) => d.parse.clone(),
            None => std::sync::Arc::new(php_syntax::parse(&text)),
        };
        let analysis = php_index::analyze(&self.index, &text, &parse.stmts);

        let mut order: Vec<SymLoc> = Vec::new();
        let mut groups: HashMap<SymLoc, Vec<Range>> = HashMap::new();
        for r in &analysis.refs {
            if !sym.range.contains(r.span.start as usize) || r.span == sym.name_span {
                continue;
            }
            let target = match &r.target {
                RefTarget::Function(_) | RefTarget::Member { kind: MemberKind::Method, .. } => {
                    self.index.resolve(&r.target).first().copied()
                }
                RefTarget::Class(fqn) if preceded_by_new(&text, r.span) => {
                    self.index.find_member(fqn, "__construct", MemberKind::Method)
                }
                _ => None,
            };
            let Some(t) = target else { continue };
            groups.entry(t).or_insert_with(|| {
                order.push(t);
                Vec::new()
            });
            groups.get_mut(&t).unwrap().push(range(&text, &lines, ident_span(&text, r.span)));
        }

        let mut out = Vec::new();
        for t in order {
            if let Some(to) = self.call_item(&mut texts, t) {
                out.push(CallHierarchyOutgoingCall { to, from_ranges: groups.remove(&t).unwrap_or_default() });
            }
        }
        serde_json::to_value(out).ok()
    }
}
