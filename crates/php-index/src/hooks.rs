//! WordPress hooks: where an action / filter is fired (`do_action('init')`,
//! `apply_filters('the_title', …)`) and where a callback is added to it (`add_action('init', …)`).
//! Collected per file at index time, so completion and go to definition need no scan.

use crate::istr::IStr;
use php_syntax::walk::{walk_expr, walk_stmts, Visitor};
use php_syntax::{Expr, ExprKind, Span, Stmt};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HookKind {
    Action,
    Filter,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HookRole {
    /// `do_action` / `apply_filters`: the hook is defined here.
    Fire,
    /// `add_action` / `add_filter`: a callback runs on the hook.
    Add,
    /// `remove_action`, `has_filter`, `did_action` …: mentions the hook.
    Other,
}

#[derive(Clone, Debug)]
pub struct Hook {
    pub name: IStr,
    pub kind: HookKind,
    pub role: HookRole,
    /// The string literal, quotes included.
    pub span: Span,
}

/// Kind and role of a hook function, by its (lowercase) name.
pub fn hook_function(name: &str) -> Option<(HookKind, HookRole)> {
    use HookKind::*;
    use HookRole::*;
    Some(match name {
        "do_action" | "do_action_ref_array" | "do_action_deprecated" => (Action, Fire),
        "apply_filters" | "apply_filters_ref_array" | "apply_filters_deprecated" => (Filter, Fire),
        "add_action" => (Action, Add),
        "add_filter" => (Filter, Add),
        "remove_action" | "remove_all_actions" | "has_action" | "did_action" | "doing_action" => (Action, Other),
        "remove_filter" | "remove_all_filters" | "has_filter" | "did_filter" | "doing_filter" => (Filter, Other),
        _ => return None,
    })
}

struct Collect<'s> {
    src: &'s str,
    out: Vec<Hook>,
}

impl<'a> Visitor<'a> for Collect<'_> {
    fn visit_expr(&mut self, e: &'a Expr) {
        if let ExprKind::Call { callee, args } = &e.kind {
            if let ExprKind::Name(n) = &callee.kind {
                let l = n.text.trim_start_matches('\\').to_ascii_lowercase();
                if let Some((kind, role)) = hook_function(&l) {
                    if let Some(a) = args.first().filter(|a| !a.spread) {
                        if let Some(name) = a.value.string_value(self.src).filter(|s| !s.is_empty() && !s.contains('$'))
                        {
                            self.out.push(Hook { name: IStr::new(name), kind, role, span: a.value.span });
                        }
                    }
                }
            }
        }
        walk_expr(self, e);
    }
}

/// Hooks fired / used in a file. Only files that call a hook function pay for the walk.
pub fn collect(src: &str, stmts: &[Stmt]) -> Vec<Hook> {
    if !(src.contains("_action") || src.contains("_filter")) {
        return Vec::new();
    }
    let mut c = Collect { src, out: Vec::new() };
    walk_stmts(&mut c, stmts);
    c.out
}
