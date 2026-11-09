use crate::convert::*;
use crate::server::Server;
use lsp_types::*;
use php_index::{docblock, RefTarget, SymLoc, SymbolKind};
use serde_json::Value;

impl Server {
    pub fn hover(&mut self, p: HoverParams) -> Option<Value> {
        let pos = p.text_document_position_params;
        let uri = canon(&pos.text_document.uri);
        let offset = {
            let doc = self.docs.get(&uri)?;
            offset_of(&doc.text, &doc.lines, pos.position)
        };
        if let Some(h) = self.laravel_hover(&uri, offset).or_else(|| self.hook_hover(&uri, offset)) {
            return serde_json::to_value(h).ok();
        }
        let analysis = self.analyze_doc(&uri)?;
        let r = analysis.ref_at(offset)?.clone();

        let md = match &r.target {
            RefTarget::Variable { name, classes, decl } => {
                let ty = if classes.is_empty() {
                    // no class: a scalar / array type where the variable got its value
                    let scalar = decl.and_then(|d| {
                        let doc = self.docs.get(&uri)?;
                        super::scalar::scalar_types(&doc.text, &doc.parse.stmts, &analysis, &self.index)
                            .remove(&d.start)
                    });
                    scalar.map(|t| format!(": {t}")).unwrap_or_default()
                } else {
                    format!(": {}", classes.join("|"))
                };
                format!("```php\n${name}{ty}\n```")
            }
            t => {
                let mut targets = self.index.resolve(t);
                if targets.is_empty() {
                    if let RefTarget::Class(fqn) = t {
                        if self.autoload(fqn) {
                            targets = self.index.resolve(t);
                        }
                    }
                }
                if targets.is_empty() {
                    return None;
                }
                targets.iter().take(3).map(|l| self.symbol_markdown(*l)).collect::<Vec<_>>().join("\n\n---\n\n")
            }
        };
        let doc = self.docs.get(&uri)?;
        let hover = Hover {
            contents: HoverContents::Markup(MarkupContent { kind: MarkupKind::Markdown, value: md }),
            range: Some(range(&doc.text, &doc.lines, r.span)),
        };
        serde_json::to_value(hover).ok()
    }

    pub fn symbol_markdown(&self, loc: SymLoc) -> String {
        let sym = self.index.sym(loc);
        let mut md = String::from("```php\n");
        if sym.kind.is_class_like() || matches!(sym.kind, SymbolKind::Function | SymbolKind::Const) {
            if let Some((ns, _)) = sym.fqn.rsplit_once('\\') {
                md.push_str(&format!("namespace {ns};\n\n"));
            }
        }
        md.push_str(&sym.detail);
        md.push_str("\n```\n");
        match &sym.container {
            Some(c) => md.push_str(&format!("*{}::{}*\n", c, sym.name)),
            None => md.push_str(&format!("*{}*\n", sym.fqn)),
        }
        if sym.deprecated {
            md.push_str("\n**Deprecated**\n");
        }
        if let Some(span) = sym.doc {
            if let Some(text) = self.text_for(self.index.uri_of(loc)) {
                let raw = span.text(&text);
                md.push('\n');
                md.push_str(&render_doc(raw));
            }
        }
        md
    }
}

fn render_doc(raw: &str) -> String {
    let d = docblock::parse(raw);
    let mut out = String::new();
    if !d.text.is_empty() {
        out.push_str(&d.text);
        out.push_str("\n\n");
    }
    for t in &d.tags {
        match t.name.as_str() {
            "param" => {
                let (ty, name) = docblock::parse_typed_var(&t.body);
                let desc = t.body.splitn(3, char::is_whitespace).nth(2).unwrap_or("").trim();
                let name = name.map(|n| format!("${n}")).unwrap_or_default();
                out.push_str(&format!("*@param* `{} {}` {}\n\n", ty, name, desc).replace("`  ", "`"));
            }
            "return" | "returns" => {
                let (ty, desc) = docblock::split_type(&t.body);
                out.push_str(&format!("*@return* `{ty}` {desc}\n\n"));
            }
            "throws" => {
                let (ty, desc) = docblock::split_type(&t.body);
                out.push_str(&format!("*@throws* `{ty}` {desc}\n\n"));
            }
            "deprecated" => out.push_str(&format!("*@deprecated* {}\n\n", t.body)),
            "see" | "link" => out.push_str(&format!("*@{}* {}\n\n", t.name, t.body)),
            _ => {}
        }
    }
    out.trim_end().to_string()
}
