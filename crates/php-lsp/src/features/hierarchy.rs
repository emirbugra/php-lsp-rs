use super::references::Target;
use crate::convert::*;
use crate::server::Server;
use crate::texts::Texts;
use lsp_types::*;
use php_index::{strip_generic, SymLoc, SymbolKind};
use serde_json::{json, Value};

impl Server {
    fn syms_at(&mut self, p: &TextDocumentPositionParams) -> Option<Vec<SymLoc>> {
        let uri = canon(&p.text_document.uri);
        let offset = {
            let doc = self.docs.get(&uri)?;
            offset_of(&doc.text, &doc.lines, p.position)
        };
        match self.target_at(&uri, offset)?.0 {
            Target::Syms { locs, .. } => Some(locs),
            _ => None,
        }
    }

    /// Symbols declared directly in `class` that override / implement `member`.
    pub(crate) fn direct_member(&self, class: SymLoc, member: &php_index::Symbol) -> Option<SymLoc> {
        let csym = self.index.sym(class);
        let file = self.index.file(class.file)?;
        let key = csym.fqn.to_ascii_lowercase();
        for &i in file.members_of(&key) {
            let s = &file.symbols[i as usize];
            if s.kind != member.kind || s.is_virtual {
                continue;
            }
            let same = if s.kind == SymbolKind::Method {
                s.name.eq_ignore_ascii_case(&member.name)
            } else {
                s.name == member.name
            };
            if same {
                return Some(SymLoc { file: class.file, idx: i });
            }
        }
        None
    }

    pub fn implementation(&mut self, p: GotoDefinitionParams) -> Option<Value> {
        let locs = self.syms_at(&p.text_document_position_params)?;
        let first = *locs.first()?;
        let sym = self.index.sym(first).clone();
        let mut found: Vec<SymLoc> = Vec::new();
        if sym.kind.is_class_like() {
            found = self.index.descendants(&sym.fqn);
        } else if let Some(container) = sym.container.as_ref() {
            for c in self.index.descendants(container) {
                if let Some(m) = self.direct_member(c, &sym) {
                    found.push(m);
                }
            }
            // A trait method is "implemented" by the trait itself.
            if found.is_empty() && !sym.is_abstract {
                found.push(first);
            }
        }
        let mut texts = Texts::default();
        let out: Vec<Location> = found.into_iter().filter_map(|l| texts.location(self, l)).collect();
        if out.is_empty() {
            return None;
        }
        serde_json::to_value(GotoDefinitionResponse::Array(out)).ok()
    }

    fn hierarchy_item(&self, texts: &mut Texts, loc: SymLoc) -> Option<TypeHierarchyItem> {
        let sym = self.index.sym(loc);
        let uri = self.index.uri_of(loc);
        let (text, lines) = texts.get(self, uri)?;
        Some(TypeHierarchyItem {
            name: sym.name.to_string(),
            kind: super::lsp_symbol_kind(sym.kind, &sym.name),
            tags: None,
            detail: Some(sym.fqn.trim_start_matches('\\').to_string()),
            uri: lsp_uri(uri),
            range: range(text, lines, sym.range),
            selection_range: range(text, lines, sym.name_span),
            data: Some(json!({ "fqn": sym.fqn.to_string() })),
        })
    }

    pub fn prepare_type_hierarchy(&mut self, p: TypeHierarchyPrepareParams) -> Option<Value> {
        let locs = self.syms_at(&p.text_document_position_params)?;
        let first = *locs.first()?;
        let sym = self.index.sym(first);
        let class = if sym.kind.is_class_like() {
            first
        } else {
            let c = sym.container.as_ref()?.to_string();
            self.index.find_class(&c)?
        };
        let item = self.hierarchy_item(&mut Texts::default(), class)?;
        serde_json::to_value(vec![item]).ok()
    }

    fn item_fqn(item: &TypeHierarchyItem) -> Option<String> {
        item.data.as_ref()?.get("fqn")?.as_str().map(|s| s.to_string())
    }

    pub fn type_supertypes(&mut self, p: TypeHierarchySupertypesParams) -> Option<Value> {
        let fqn = Self::item_fqn(&p.item)?;
        let loc = self.index.find_class(&fqn)?;
        let s = self.index.sym(loc);
        let bases: Vec<String> =
            s.extends().iter().chain(s.implements()).chain(s.traits()).map(|b| strip_generic(b).to_string()).collect();
        let mut out = Vec::new();
        let mut texts = Texts::default();
        for b in bases {
            if self.index.find_class(&b).is_none() {
                self.autoload(&b);
            }
            for l in self.index.find_classes(&b).to_vec() {
                out.extend(self.hierarchy_item(&mut texts, l));
            }
        }
        serde_json::to_value(out).ok()
    }

    pub fn type_subtypes(&mut self, p: TypeHierarchySubtypesParams) -> Option<Value> {
        let fqn = Self::item_fqn(&p.item)?;
        let mut texts = Texts::default();
        let out: Vec<TypeHierarchyItem> =
            self.index.subtypes(&fqn).into_iter().filter_map(|l| self.hierarchy_item(&mut texts, l)).collect();
        serde_json::to_value(out).ok()
    }
}
