use crate::convert::*;
use crate::server::Server;
use lsp_types::*;
use php_index::{MemberKind, RefTarget, SymLoc};
use serde_json::Value;

/// Find the unmatched `(` before `offset` and the active argument index.
fn open_paren(text: &str, offset: usize) -> Option<(usize, u32)> {
    let b = text.as_bytes();
    let mut depth = 0i32;
    let mut commas = 0u32;
    let mut i = offset.min(b.len());
    let mut steps = 0;
    while i > 0 {
        i -= 1;
        steps += 1;
        if steps > 4000 {
            return None;
        }
        match b[i] {
            b')' | b']' | b'}' => depth += 1,
            b'[' | b'{' => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
            }
            b'(' => {
                if depth == 0 {
                    return Some((i, commas));
                }
                depth -= 1;
            }
            b',' if depth == 0 => commas += 1,
            b';' if depth == 0 => return None,
            _ => {}
        }
    }
    None
}

impl Server {
    pub fn signature_help(&mut self, p: SignatureHelpParams) -> Option<Value> {
        let pos = p.text_document_position_params;
        let uri = canon(&pos.text_document.uri);
        let (open, active, analysis) = {
            let doc = self.docs.get(&uri)?;
            let offset = offset_of(&doc.text, &doc.lines, pos.position);
            let (open, active) = open_paren(&doc.text, offset)?;
            (open, active, self.analyze_doc(&uri)?)
        };
        let doc = self.docs.get(&uri)?;
        let before = doc.text[..open].trim_end().len();
        let r = analysis.refs.iter().find(|r| r.span.end as usize == before)?;

        let mut locs: Vec<SymLoc> = self.index.resolve(&r.target);
        if let RefTarget::Class(fqn) = &r.target {
            // `new Foo(` -> constructor
            locs = self.index.find_member(fqn, "__construct", MemberKind::Method).into_iter().collect();
        }
        if locs.is_empty() {
            return None;
        }
        let sym = self.index.sym(locs[0]);
        if sym.params.is_empty() && !sym.detail.contains('(') {
            return None;
        }
        let params: Vec<String> = sym
            .params
            .iter()
            .map(|p| {
                let mut s = String::new();
                if let Some(t) = &p.ty {
                    s.push_str(t);
                    s.push(' ');
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
            })
            .collect();
        let label = sym.detail.to_string();
        let parameters: Vec<ParameterInformation> = params
            .iter()
            .map(|p| ParameterInformation { label: ParameterLabel::Simple(p.clone()), documentation: None })
            .collect();
        let active = if sym.params.last().map(|p| p.variadic).unwrap_or(false) {
            active.min(params.len().saturating_sub(1) as u32)
        } else {
            active
        };
        let help = SignatureHelp {
            signatures: vec![SignatureInformation {
                label,
                documentation: None,
                parameters: Some(parameters),
                active_parameter: Some(active),
            }],
            active_signature: Some(0),
            active_parameter: Some(active),
        };
        serde_json::to_value(help).ok()
    }
}
