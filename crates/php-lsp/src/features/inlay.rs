//! Inlay hints:
//! - parameter names before call arguments (`setName(name: $x)`), unless the argument already says it;
//! - the inferred type after a variable where it gets its value (`$user: User = User::find(1)`,
//!   `foreach ($users as $user: User)`, untyped closure parameters), unless the type is already
//!   written there (`new User`, a typed parameter, `catch (E $e)`, a `@var` tag).

use crate::convert::*;
use crate::server::Server;
use lsp_types::*;
use php_index::{types::short_display, Analysis, MemberKind, RefTarget};
use php_syntax::walk::{walk_expr, walk_function, walk_stmt, walk_stmts, Visitor};
use php_syntax::{Arg, Expr, ExprKind, FunctionDecl, MemberName, Span, Stmt, StmtKind};
use rustc_hash::{FxHashMap, FxHashSet};
use serde_json::Value;

struct CallSite {
    /// Byte offset right after the callee name (matches `Reference::span.end`).
    callee_end: u32,
    args: Vec<Arg>,
}

/// What the hints need from the syntax tree.
#[derive(Default)]
struct Sites {
    calls: Vec<CallSite>,
    /// Variables whose type is plain to see where they are declared (start offsets).
    obvious: FxHashSet<u32>,
}

impl<'ast> Visitor<'ast> for Sites {
    fn visit_expr(&mut self, e: &'ast Expr) {
        match &e.kind {
            ExprKind::Call { callee, args } => {
                if let ExprKind::Name(n) = &callee.kind {
                    self.calls.push(CallSite { callee_end: n.span.end, args: args.clone() });
                }
            }
            ExprKind::MethodCall { name: MemberName::Ident(i), args, .. }
            | ExprKind::StaticCall { name: MemberName::Ident(i), args, .. } => {
                self.calls.push(CallSite { callee_end: i.span.end, args: args.clone() });
            }
            ExprKind::New { class: php_syntax::ClassRef::Name(n), args } => {
                self.calls.push(CallSite { callee_end: n.span.end, args: args.clone() });
            }
            ExprKind::Assign { target, value, .. } if self_evident(value) => {
                self.obvious.insert(target.span.start);
            }
            _ => {}
        }
        walk_expr(self, e);
    }

    fn visit_stmt(&mut self, s: &'ast Stmt) {
        if let StmtKind::Try { catches, .. } = &s.kind {
            for c in catches {
                if let Some(v) = &c.var {
                    self.obvious.insert(v.span.start);
                }
            }
        }
        walk_stmt(self, s);
    }

    fn visit_function(&mut self, f: &'ast FunctionDecl) {
        for p in &f.params {
            // a declared type, or a named function's parameter (its docblock is right above)
            if p.ty.is_some() || f.name.is_some() {
                self.obvious.insert(p.span.start);
            }
        }
        walk_function(self, f);
    }
}

/// The value shows its own type: `new User`, a closure literal, `User::class`, literals and casts.
fn self_evident(value: &Expr) -> bool {
    match &value.kind {
        ExprKind::New { .. }
        | ExprKind::Closure(_)
        | ExprKind::Lit
        | ExprKind::Cast { .. }
        | ExprKind::ClassConst { .. } => true,
        ExprKind::Paren(inner) => self_evident(inner),
        _ => false,
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

/// A `@var` tag on the line before `offset` or earlier on the same line.
fn has_var_tag(src: &str, offset: usize) -> bool {
    let line_start = src[..offset].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let prev_start = src[..line_start.saturating_sub(1)].rfind('\n').map(|i| i + 1).unwrap_or(0);
    src[prev_start..offset].contains("@var")
}

fn hint(pos: Position, label: String, kind: InlayHintKind, left: bool, right: bool) -> InlayHint {
    InlayHint {
        position: pos,
        label: InlayHintLabel::String(label),
        kind: Some(kind),
        text_edits: None,
        tooltip: None,
        padding_left: Some(left),
        padding_right: Some(right),
        data: None,
    }
}

impl Server {
    pub fn inlay_hints(&mut self, p: InlayHintParams) -> Option<Value> {
        if !self.config.inlay_parameter_names && !self.config.inlay_variable_types {
            return serde_json::to_value(Vec::<InlayHint>::new()).ok();
        }
        let uri = canon(&p.text_document.uri);
        let analysis = self.analyze_doc(&uri)?;
        let doc = self.docs.get(&uri)?;
        let from = offset_of(&doc.text, &doc.lines, p.range.start);
        let to = offset_of(&doc.text, &doc.lines, p.range.end);

        let mut sites = Sites::default();
        walk_stmts(&mut sites, &doc.parse.stmts);
        let mut hints = Vec::new();
        if self.config.inlay_parameter_names {
            self.parameter_hints(&analysis, &sites, doc, from, to, &mut hints);
        }
        if self.config.inlay_variable_types {
            variable_type_hints(&analysis, &sites, doc, from, to, &mut hints);
        }
        hints.sort_by_key(|h| (h.position.line, h.position.character));
        serde_json::to_value(hints).ok()
    }

    fn parameter_hints(
        &self,
        analysis: &Analysis,
        sites: &Sites,
        doc: &crate::server::Doc,
        from: usize,
        to: usize,
        hints: &mut Vec<InlayHint>,
    ) {
        let by_end: FxHashMap<u32, &RefTarget> = analysis.refs.iter().map(|r| (r.span.end, &r.target)).collect();
        for call in &sites.calls {
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
                let (line, character) = doc.lines.position(&doc.text, at.start as usize);
                let label = if param.variadic { format!("...{}:", param.name) } else { format!("{}:", param.name) };
                hints.push(hint(Position { line, character }, label, InlayHintKind::PARAMETER, false, true));
            }
        }
    }
}

fn variable_type_hints(
    analysis: &Analysis,
    sites: &Sites,
    doc: &crate::server::Doc,
    from: usize,
    to: usize,
    hints: &mut Vec<InlayHint>,
) {
    for r in &analysis.refs {
        let RefTarget::Variable { name, decl: Some(decl), classes } = &r.target else { continue };
        // only where the variable gets its value, not where it is read
        if *decl != r.span || (r.span.start as usize) < from || (r.span.start as usize) > to || name == "this" {
            continue;
        }
        if sites.obvious.contains(&r.span.start) || has_var_tag(&doc.text, r.span.start as usize) {
            continue;
        }
        let Some(shown) = short_display(classes) else { continue };
        let (line, character) = doc.lines.position(&doc.text, r.span.end as usize);
        hints.push(hint(Position { line, character }, format!(": {shown}"), InlayHintKind::TYPE, false, false));
    }
}
