//! Code lenses above declarations:
//! - "N references" on classes, interfaces, traits, enums, functions and methods, counted in the
//!   project's own files (not `vendor/`). They are counted in `codeLens/resolve`, so only the
//!   lenses on screen cost a search;
//! - "N implementations" on interfaces and interface / abstract methods, "N subclasses" on classes
//!   and "N overrides" on methods, shown only when there are any.
//!
//! Clicking a lens runs the client command `phpLsp.showReferences(uri, position, locations)`.

use super::references::Target;
use crate::convert::*;
use crate::server::Server;
use crate::texts::Texts;
use lsp_types::*;
use php_index::{SymLoc, Symbol, SymbolKind};
use serde_json::{json, Value};

const SHOW: &str = "phpLsp.showReferences";

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn wants_references(s: &Symbol) -> bool {
    s.kind.is_class_like()
        || s.kind == SymbolKind::Function
        || (s.kind == SymbolKind::Method && !s.name.starts_with("__"))
}

impl Server {
    pub fn code_lens(&mut self, p: CodeLensParams) -> Option<Value> {
        let uri = canon(&p.text_document.uri);
        if !self.config.code_lens || self.index.is_library(&uri) {
            return serde_json::to_value(Vec::<CodeLens>::new()).ok();
        }
        let doc = self.docs.get(&uri)?;
        let file = self.index.file_by_uri(&uri)?.clone();
        let fid = self.index.file_id(&uri)?;
        let subtypes = self.index.subtype_map();
        let mut texts = Texts::default();
        let mut lenses = Vec::new();
        for (i, s) in file.symbols.iter().enumerate() {
            if s.is_virtual || s.name_span.is_empty() {
                continue;
            }
            let at = range(&doc.text, &doc.lines, s.name_span);
            if wants_references(s) {
                lenses.push(CodeLens {
                    range: at,
                    command: None,
                    data: Some(json!({ "uri": uri, "offset": s.name_span.start })),
                });
            }
            let loc = SymLoc { file: fid, idx: i as u32 };
            let (found, one, many) = if s.kind.is_class_like() {
                if s.kind == SymbolKind::Trait {
                    continue;
                }
                let found = self.index.descendants_with(&subtypes, &s.fqn);
                if s.kind == SymbolKind::Interface {
                    (found, "implementation", "implementations")
                } else {
                    (found, "subclass", "subclasses")
                }
            } else if s.kind == SymbolKind::Method && !s.name.eq_ignore_ascii_case("__construct") {
                let Some(container) = s.container.as_ref() else { continue };
                let found: Vec<SymLoc> = self
                    .index
                    .descendants_with(&subtypes, container)
                    .into_iter()
                    .filter_map(|c| self.direct_member(c, s))
                    .filter(|m| *m != loc)
                    .collect();
                if s.is_abstract {
                    (found, "implementation", "implementations")
                } else {
                    (found, "override", "overrides")
                }
            } else {
                continue;
            };
            if found.is_empty() {
                continue;
            }
            let locations: Vec<Location> = found.iter().filter_map(|l| texts.location(self, *l)).collect();
            lenses.push(CodeLens {
                range: at,
                command: Some(Command {
                    title: plural(found.len(), one, many),
                    command: SHOW.into(),
                    arguments: Some(vec![json!(uri), json!(at.start), json!(locations)]),
                }),
                data: None,
            });
        }
        serde_json::to_value(lenses).ok()
    }

    pub fn code_lens_resolve(&mut self, mut lens: CodeLens) -> Option<Value> {
        let data = lens.data.take()?;
        let uri = data.get("uri")?.as_str()?.to_string();
        let offset = data.get("offset")?.as_u64()? as usize;
        let hits = match self.target_at(&uri, offset) {
            Some((target @ Target::Syms { .. }, _)) => {
                let Target::Syms { locs, .. } = &target else { unreachable!() };
                let decl_files: Vec<String> = locs.iter().map(|l| self.index.uri_of(*l).to_string()).collect();
                let private_here = locs.iter().all(|l| {
                    let s = self.index.sym(*l);
                    s.vis == php_syntax::Visibility::Private
                        && s.container.as_ref().is_some_and(|c| {
                            self.index.find_class(c).is_some_and(|cl| self.index.sym(cl).kind != SymbolKind::Trait)
                        })
                });
                let keep = |u: &str| {
                    if private_here {
                        decl_files.iter().any(|d| d == u)
                    } else {
                        !self.index.is_library(u)
                    }
                };
                self.find_hits_in(&uri, &target, false, false, &keep)
            }
            _ => Vec::new(),
        };
        let locations: Vec<Location> =
            hits.into_iter().map(|h| Location { uri: lsp_uri(&h.uri), range: h.range }).collect();
        lens.command = Some(Command {
            title: plural(locations.len(), "reference", "references"),
            command: if locations.is_empty() { String::new() } else { SHOW.into() },
            arguments: Some(vec![json!(uri), json!(lens.range.start), json!(locations)]),
        });
        serde_json::to_value(lens).ok()
    }
}
