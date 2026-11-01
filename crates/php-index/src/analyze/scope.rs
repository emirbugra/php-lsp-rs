//! Variables of the current function and class names as written in the source.

use super::{push_unique, Analyzer, ClassScope, RefTarget, Reference, Scope, ScopeSnapshot, VarInfo};
use crate::index::MemberKind;
use crate::names::is_builtin_type;
use crate::types::{self, Types};
use php_syntax::walk::{walk_stmts, Visitor};
use php_syntax::*;

impl Analyzer<'_> {
    /// Store the variables of a finished scope in the result (used for completion of `$`).
    pub(super) fn snapshot(&mut self, s: Scope) {
        let mut vars: Vec<(String, Types)> = s.vars.into_iter().map(|(k, v)| (k, v.classes)).collect();
        vars.sort();
        self.out.scopes.push(ScopeSnapshot { span: s.span, vars });
    }

    pub(super) fn current_class(&self) -> Option<&ClassScope> {
        self.classes.last()
    }

    /// Resolve a class name as written, honouring self/static/parent.
    pub(super) fn resolve_class_name(&self, name: &str) -> Option<String> {
        match name.to_ascii_lowercase().as_str() {
            "self" | "static" => self.current_class().map(|c| c.fqn.clone()),
            "parent" => self.current_class().and_then(|c| c.parent.clone()),
            _ => Some(self.ctx.resolve_class(name)),
        }
    }

    /// Resolve a class name and record it as a reference (except self/static/parent).
    pub(super) fn class_name_ref(&mut self, n: &Name) -> Types {
        let Some(fqn) = self.resolve_class_name(&n.text) else { return Vec::new() };
        let special = matches!(n.text.to_ascii_lowercase().as_str(), "self" | "static" | "parent");
        if !special {
            self.out.refs.push(Reference { span: n.span, target: RefTarget::Class(fqn.clone()) });
        }
        vec![fqn]
    }

    /// Classes named by `Foo` / `$cls` / `new class {}` in `new X`, `X::y` or `instanceof X`.
    pub(super) fn class_ref_classes(&mut self, c: &ClassRef) -> Types {
        match c {
            ClassRef::Name(n) => self.class_name_ref(n),
            ClassRef::Expr(e) => {
                self.visit_expr(e);
                self.only_classes(self.infer(e))
            }
            ClassRef::Anon(c) => {
                self.visit_class(c);
                Vec::new()
            }
        }
    }

    /// Record the class names of a type declaration (`function f(User $u): ?Post`) as references.
    pub(super) fn type_refs(&mut self, h: &TypeHint) {
        for n in &h.names {
            if is_builtin_type(&n.text) || matches!(n.text.to_ascii_lowercase().as_str(), "self" | "static" | "parent")
            {
                continue;
            }
            let fqn = self.ctx.resolve_class(&n.text);
            self.out.refs.push(Reference { span: n.span, target: RefTarget::Class(fqn) });
        }
    }

    /// Types of a type declaration.
    pub(super) fn hint_classes(&self, h: &TypeHint) -> Types {
        let mut out = Vec::new();
        for n in &h.names {
            if is_builtin_type(&n.text) {
                continue;
            }
            if let Some(c) = self.resolve_class_name(&n.text) {
                push_unique(&mut out, c);
            }
        }
        out
    }

    /// Types of a docblock type (`User[]|null`).
    pub(super) fn doc_classes(&self, ty: &str) -> Types {
        types::parse_doc_type(ty, &|n| match n.to_ascii_lowercase().as_str() {
            "static" | "self" => self.current_class().map(|c| c.fqn.clone()),
            "parent" => self.current_class().and_then(|c| c.parent.clone()),
            _ if is_builtin_type(n) => None,
            _ => Some(self.ctx.resolve_class(n)),
        })
        .into_iter()
        .map(|a| if a == "static" { self.current_class().map(|c| c.fqn.clone()).unwrap_or(a) } else { a })
        .collect()
    }

    /// Set the types of a variable; returns where it was first declared.
    pub(super) fn declare(&mut self, name: &str, span: Span, classes: Types) -> Span {
        let scope = self.scopes.last_mut().unwrap();
        match scope.vars.get_mut(name) {
            Some(v) => {
                v.classes = classes;
                v.decl
            }
            None => {
                scope.vars.insert(name.to_string(), VarInfo { classes, decl: span });
                span
            }
        }
    }

    pub(super) fn lookup(&self, name: &str) -> Option<&VarInfo> {
        self.scopes.last().and_then(|s| s.vars.get(name))
    }

    /// `$name = …` (or `$name .= …` etc. when `op` is not `=`): declare and record a reference.
    pub(super) fn assign_var(&mut self, name: &str, span: Span, classes: Types, op: &str) {
        if self.narrowing_mark() > 0 {
            self.forget_narrowing(&format!("${name}"));
        }
        let mut classes = self.pending_doc_type.take().unwrap_or(classes);
        if let Some((_, dc)) = self.doc_vars.iter().find(|(n, _)| n == name) {
            classes = dc.clone();
        }
        if op != "=" {
            if let Some(v) = self.lookup(name) {
                for c in &v.classes {
                    push_unique(&mut classes, c.clone());
                }
            }
        }
        let decl = self.declare(name, span, classes.clone());
        self.out.refs.push(Reference {
            span,
            target: RefTarget::Variable { name: name.to_string(), decl: Some(decl), classes },
        });
    }

    /// Destructuring / foreach targets; `atoms` are the types being assigned to `e`.
    pub(super) fn declare_targets(&mut self, e: &Expr, atoms: &[String]) {
        match &e.kind {
            ExprKind::Var(n) => self.assign_var(n, e.span, atoms.to_vec(), "="),
            ExprKind::Array(items) => {
                let sub = self.element_types_of_all(atoms);
                for i in items {
                    self.declare_targets(&i.value, &sub);
                }
            }
            _ => self.visit_expr(e),
        }
    }

    /// `$arr[] = new A` makes `$arr` an `A[]`.
    pub(super) fn add_var_types(&mut self, name: &str, span: Span, atoms: Types) {
        let mut merged = self.lookup(name).map(|v| v.classes.clone()).unwrap_or_default();
        for a in atoms {
            push_unique(&mut merged, a);
        }
        self.declare(name, span, merged);
    }

    pub(super) fn push_member_ref(&mut self, span: Span, kind: MemberKind, name: &str, classes: Types) {
        if name.is_empty() {
            return;
        }
        self.out.refs.push(Reference { span, target: RefTarget::Member { kind, name: name.to_string(), classes } });
    }

    /// Walk a block that may or may not run (if / loop / try body).
    pub(super) fn walk_branch(&mut self, stmts: &[Stmt]) {
        self.branch_depth += 1;
        walk_stmts(self, stmts);
        self.branch_depth -= 1;
    }
}
