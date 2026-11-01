use crate::convert::*;
use crate::server::Server;
use lsp_types::*;
use php_index::{MemberKind, RefTarget};
use php_syntax::walk::{walk_expr, walk_stmts, Visitor};
use php_syntax::{Arg, Expr, ExprKind, MemberName, Span};
use rustc_hash::FxHashMap;
use serde_json::Value;

struct CallSite {
    /// Byte offset right after the callee name (matches `Reference::span.end`).
    callee_end: u32,
    args: Vec<Arg>,
}

#[derive(Default)]
struct Calls(Vec<CallSite>);

impl<'ast> Visitor<'ast> for Calls {
    fn visit_expr(&mut self, e: &'ast Expr) {
        match &e.kind {
            ExprKind::Call { callee, args } => {
                if let ExprKind::Name(n) = &callee.kind {
                    self.0.push(CallSite { callee_end: n.span.end, args: args.clone() });
                }
            }
            ExprKind::MethodCall { name: MemberName::Ident(i), args, .. }
            | ExprKind::StaticCall { name: MemberName::Ident(i), args, .. } => {
                self.0.push(CallSite { callee_end: i.span.end, args: args.clone() });
            }
            ExprKind::New { class: php_syntax::ClassRef::Name(n), args } => {
                self.0.push(CallSite { callee_end: n.span.end, args: args.clone() });
            }
            _ => {}
        }
        walk_expr(self, e);
    }
}

/// Identifier an argument "says" about itself: `$user` -> user, `$this->user` / `$o->user()` -> user.
fn arg_name(e: &Expr) -> Option<&str> {
    match &e.kind {
        ExprKind::Var(n) => Some(n.trim_start_matches('$')),
        ExprKind::Prop { name: MemberName::Ident(i), .. } | ExprKind::MethodCall { name: MemberName::Ident(i), .. } => {
            Some(&i.text)
        }
        ExprKind::Paren(inner) => arg_name(inner),
        _ => None,
    }
}

impl Server {
    pub fn inlay_hints(&mut self, p: InlayHintParams) -> Option<Value> {
        let uri = canon(&p.text_document.uri);
        let analysis = self.analyze_doc(&uri)?;
        let doc = self.docs.get(&uri)?;
        let from = offset_of(&doc.text, &doc.lines, p.range.start);
        let to = offset_of(&doc.text, &doc.lines, p.range.end);

        let mut calls = Calls::default();
        walk_stmts(&mut calls, &doc.parse.stmts);

        let by_end: FxHashMap<u32, &RefTarget> = analysis.refs.iter().map(|r| (r.span.end, &r.target)).collect();
        let mut hints = Vec::new();
        for call in &calls.0 {
            if call.args.is_empty() {
                continue;
            }
            let (Some(first), Some(last)) = (call.args.first(), call.args.last()) else { continue };
            if (last.value.span.end as usize) < from || (first.value.span.start as usize) > to {
                continue;
            }
            let Some(target) = by_end.get(&call.callee_end) else { continue };
            let loc = match target {
                RefTarget::Class(fqn) => self.index.find_member(fqn, "__construct", MemberKind::Method),
                t => self.index.resolve(t).first().copied(),
            };
            let Some(loc) = loc else { continue };
            let sym = self.index.sym(loc);
            if sym.params.is_empty() {
                continue;
            }
            for (i, arg) in call.args.iter().enumerate() {
                if arg.name.is_some() || arg.spread {
                    continue;
                }
                let param = match sym.params.get(i) {
                    Some(p) => p,
                    None => match sym.params.last() {
                        Some(l) if l.variadic => continue,
                        _ => break,
                    },
                };
                if sym.params.len() == 1
                    && !param.variadic
                    && arg_name(&arg.value).is_none()
                    && matches!(arg.value.kind, ExprKind::Array(_) | ExprKind::Call { .. } | ExprKind::New { .. })
                {
                    continue;
                }
                if arg_name(&arg.value).map(|n| n.eq_ignore_ascii_case(&param.name)).unwrap_or(false) {
                    continue;
                }
                let at: Span = arg.value.span;
                let (line, col) = doc.lines.position(&doc.text, at.start as usize);
                let label = if param.variadic { format!("...{}:", param.name) } else { format!("{}:", param.name) };
                hints.push(InlayHint {
                    position: Position { line, character: col },
                    label: InlayHintLabel::String(label),
                    kind: Some(InlayHintKind::PARAMETER),
                    text_edits: None,
                    tooltip: None,
                    padding_left: Some(false),
                    padding_right: Some(true),
                    data: None,
                });
            }
        }
        serde_json::to_value(hints).ok()
    }
}
