//! Generic AST traversal.

use crate::ast::*;

/// `'ast` is the lifetime of the syntax tree, so a visitor may keep references to the nodes it
/// sees (e.g. nested functions to look at later).
pub trait Visitor<'ast>: Sized {
    fn visit_stmt(&mut self, s: &'ast Stmt) {
        walk_stmt(self, s)
    }
    fn visit_expr(&mut self, e: &'ast Expr) {
        walk_expr(self, e)
    }
    fn visit_class(&mut self, c: &'ast ClassDecl) {
        walk_class(self, c)
    }
    fn visit_function(&mut self, f: &'ast FunctionDecl) {
        walk_function(self, f)
    }
}

pub fn walk_stmts<'ast, V: Visitor<'ast>>(v: &mut V, stmts: &'ast [Stmt]) {
    for s in stmts {
        v.visit_stmt(s);
    }
}

pub fn walk_stmt<'ast, V: Visitor<'ast>>(v: &mut V, s: &'ast Stmt) {
    match &s.kind {
        StmtKind::InlineHtml | StmtKind::Nop | StmtKind::Use(_) => {}
        StmtKind::Exprs(es) => es.iter().for_each(|e| v.visit_expr(e)),
        StmtKind::Expr(e) => v.visit_expr(e),
        StmtKind::Namespace { body, .. } => {
            if let Some(b) = body {
                walk_stmts(v, b)
            }
        }
        StmtKind::Function(f) => v.visit_function(f),
        StmtKind::Class(c) => v.visit_class(c),
        StmtKind::Const(items) => items.iter().for_each(|i| v.visit_expr(&i.value)),
        StmtKind::Control { header, blocks, .. } => {
            header.iter().for_each(|e| v.visit_expr(e));
            blocks.iter().for_each(|b| walk_stmts(v, b));
        }
        StmtKind::Try { body, catches, finally } => {
            walk_stmts(v, body);
            for c in catches {
                walk_stmts(v, &c.body);
            }
            if let Some(f) = finally {
                walk_stmts(v, f);
            }
        }
        StmtKind::Block(b) => walk_stmts(v, b),
    }
}

pub fn walk_class<'ast, V: Visitor<'ast>>(v: &mut V, c: &'ast ClassDecl) {
    c.attrs.iter().for_each(|e| v.visit_expr(e));
    for m in &c.members {
        match m {
            Member::Method { decl, .. } => v.visit_function(decl),
            Member::Property(p) => {
                if let Some(d) = &p.default {
                    v.visit_expr(d)
                }
            }
            Member::Const { items, .. } => items.iter().for_each(|i| v.visit_expr(&i.value)),
            Member::Case { value, .. } => {
                if let Some(e) = value {
                    v.visit_expr(e)
                }
            }
            Member::TraitUse { .. } => {}
        }
    }
}

pub fn walk_function<'ast, V: Visitor<'ast>>(v: &mut V, f: &'ast FunctionDecl) {
    f.attrs.iter().for_each(|e| v.visit_expr(e));
    for p in &f.params {
        p.attrs.iter().for_each(|e| v.visit_expr(e));
        if let Some(d) = &p.default {
            v.visit_expr(d)
        }
    }
    if let Some(b) = &f.body {
        walk_stmts(v, b);
    }
    if let Some(e) = &f.arrow_body {
        v.visit_expr(e);
    }
}

fn walk_args<'ast, V: Visitor<'ast>>(v: &mut V, args: &'ast [Arg]) {
    args.iter().for_each(|a| v.visit_expr(&a.value));
}

fn walk_member<'ast, V: Visitor<'ast>>(v: &mut V, m: &'ast MemberName) {
    if let MemberName::Expr(e) = m {
        v.visit_expr(e)
    }
}

fn walk_class_ref<'ast, V: Visitor<'ast>>(v: &mut V, c: &'ast ClassRef) {
    match c {
        ClassRef::Name(_) => {}
        ClassRef::Expr(e) => v.visit_expr(e),
        ClassRef::Anon(c) => v.visit_class(c),
    }
}

pub fn walk_expr<'ast, V: Visitor<'ast>>(v: &mut V, e: &'ast Expr) {
    match &e.kind {
        ExprKind::Var(_) | ExprKind::Name(_) | ExprKind::Lit | ExprKind::Error => {}
        ExprKind::VarVar(x)
        | ExprKind::Paren(x)
        | ExprKind::Empty(x)
        | ExprKind::Include(x)
        | ExprKind::Clone(x)
        | ExprKind::Print(x)
        | ExprKind::Throw(x)
        | ExprKind::YieldFrom(x) => v.visit_expr(x),
        ExprKind::Interp(parts) | ExprKind::Isset(parts) => parts.iter().for_each(|p| v.visit_expr(p)),
        ExprKind::Array(items) => {
            for i in items {
                if let Some(k) = &i.key {
                    v.visit_expr(k)
                }
                v.visit_expr(&i.value)
            }
        }
        ExprKind::Call { callee, args } => {
            v.visit_expr(callee);
            walk_args(v, args);
        }
        ExprKind::New { class, args } => {
            walk_class_ref(v, class);
            walk_args(v, args);
        }
        ExprKind::MethodCall { obj, name, args, .. } => {
            v.visit_expr(obj);
            walk_member(v, name);
            walk_args(v, args);
        }
        ExprKind::Prop { obj, name, .. } => {
            v.visit_expr(obj);
            walk_member(v, name);
        }
        ExprKind::StaticCall { class, name, args } => {
            walk_class_ref(v, class);
            walk_member(v, name);
            walk_args(v, args);
        }
        ExprKind::StaticProp { class, name } => {
            walk_class_ref(v, class);
            walk_member(v, name);
        }
        ExprKind::ClassConst { class, .. } => walk_class_ref(v, class),
        ExprKind::Index { base, index } => {
            v.visit_expr(base);
            if let Some(i) = index {
                v.visit_expr(i)
            }
        }
        ExprKind::Binary { lhs, rhs, .. } => {
            v.visit_expr(lhs);
            v.visit_expr(rhs);
        }
        ExprKind::Unary { expr, .. } | ExprKind::Cast { expr, .. } => v.visit_expr(expr),
        ExprKind::Assign { target, value, .. } => {
            v.visit_expr(value);
            v.visit_expr(target);
        }
        ExprKind::Ternary { cond, then, els } => {
            v.visit_expr(cond);
            if let Some(t) = then {
                v.visit_expr(t)
            }
            v.visit_expr(els);
        }
        ExprKind::Closure(f) => v.visit_function(f),
        ExprKind::Match { subject, arms } => {
            v.visit_expr(subject);
            for a in arms {
                a.conds.iter().for_each(|c| v.visit_expr(c));
                v.visit_expr(&a.body);
            }
        }
        ExprKind::InstanceOf { expr, class } => {
            v.visit_expr(expr);
            walk_class_ref(v, class);
        }
        ExprKind::Exit(x) => {
            if let Some(x) = x {
                v.visit_expr(x)
            }
        }
        ExprKind::Yield { key, value } => {
            if let Some(k) = key {
                v.visit_expr(k)
            }
            if let Some(x) = value {
                v.visit_expr(x)
            }
        }
    }
}
