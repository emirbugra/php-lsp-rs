use crate::convert::*;
use crate::features::lsp_symbol_kind;
use crate::server::Server;
use crate::texts::Texts;
use lsp_types::*;
use php_index::Symbol;
use serde_json::Value;

impl Server {
    pub fn document_symbols(&mut self, p: DocumentSymbolParams) -> Option<Value> {
        let uri = canon(&p.text_document.uri);
        let doc = self.docs.get(&uri)?;
        let file = self.index.file_by_uri(&uri)?;
        let mk = |s: &Symbol, children: Option<Vec<DocumentSymbol>>| -> DocumentSymbol {
            #[allow(deprecated)]
            DocumentSymbol {
                name: s.name.to_string(),
                detail: Some(s.detail.to_string()),
                kind: lsp_symbol_kind(s.kind, &s.name),
                tags: if s.deprecated { Some(vec![SymbolTag::DEPRECATED]) } else { None },
                deprecated: None,
                range: range(&doc.text, &doc.lines, s.range),
                selection_range: range(&doc.text, &doc.lines, s.name_span),
                children,
            }
        };
        let mut out = Vec::new();
        for s in &file.symbols {
            if s.container.is_some() {
                continue;
            }
            if s.kind.is_class_like() {
                let children: Vec<DocumentSymbol> = file
                    .symbols
                    .iter()
                    .filter(|m| !m.is_virtual && m.container.as_deref() == Some(s.fqn.as_str()))
                    .map(|m| mk(m, None))
                    .collect();
                out.push(mk(s, Some(children)));
            } else {
                out.push(mk(s, None));
            }
        }
        serde_json::to_value(DocumentSymbolResponse::Nested(out)).ok()
    }

    pub fn workspace_symbols(&mut self, p: WorkspaceSymbolParams) -> Option<Value> {
        let hits = self.index.search(&p.query, 80);
        let mut out = Vec::new();
        let mut texts = Texts::default();
        for loc in hits {
            let sym = self.index.sym(loc);
            let uri = self.index.uri_of(loc);
            let Some(range) = texts.range(self, uri, sym.name_span) else { continue };
            #[allow(deprecated)]
            out.push(SymbolInformation {
                name: sym.name.to_string(),
                kind: lsp_symbol_kind(sym.kind, &sym.name),
                tags: if sym.deprecated { Some(vec![SymbolTag::DEPRECATED]) } else { None },
                deprecated: None,
                location: Location { uri: lsp_uri(uri), range },
                container_name: sym.fqn.rsplit_once('\\').map(|(ns, _)| ns.to_string()),
            });
        }
        serde_json::to_value(WorkspaceSymbolResponse::Flat(out)).ok()
    }
}
