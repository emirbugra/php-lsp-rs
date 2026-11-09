//! The tree walk: records references and updates variable types statement by statement.

use super::hints::ArgHints;
use super::{push_unique, Analyzer, ClassScope, ParamTypes, RefTarget, Reference, Scope, Types, VarInfo};
use crate::docblock;
use crate::index::MemberKind;
use php_syntax::walk::{walk_expr, walk_stmt, walk_stmts, Visitor};
use php_syntax::*;
use rustc_hash::FxHashMap;

impl<'ast> Visitor<'ast> for Analyzer<'_> {
    fn visit_stmt(&mut self, s: &'ast Stmt) {
        self.pending_doc_type = None;
        self.doc_vars.clear();
        self.handle_doc_vars(s);
        self.doc_refs(s.doc);
        match &s.kind {
            StmtKind::Namespace { name, body } => {
                self.ctx.set_namespace(name.as_ref().map(|n| n.text.as_str()).unwrap_or(""));
                if let Some(b) = body {
                    walk_stmts(self, b);
                    self.ctx.set_namespace("");
                }
            }
            StmtKind::Use(items) => self.visit_use(items),
            StmtKind::Try { body, catches, finally } => self.visit_try(body, catches, finally.as_deref()),
            StmtKind::Control { kw: "foreach", header, blocks } => self.visit_foreach(header, blocks),
            StmtKind::Control { kw: "if", header, blocks } => self.visit_if(header, blocks),
            StmtKind::Control { kw: "while", header, blocks } => self.visit_while(header, blocks),
            StmtKind::Control { kw: "for" | "do" | "switch", .. } => {
                self.branch_depth += 1;
                walk_stmt(self, s);
                self.branch_depth -= 1;
            }
            _ => {
                walk_stmt(self, s);
                if let Some(Some(value)) = s.return_value(self.src) {
                    let classes = self.infer(value);
                    self.out.returns.push((value.span, classes));
                }
            }
        }
    }

    fn visit_class(&mut self, c: &'ast ClassDecl) {
        let parent = c.extends.first().map(|e| self.ctx.resolve_class(&e.text));
        let fqn = match &c.name {
            Some(n) if self.ctx.namespace.is_empty() => n.text.clone(),
            Some(n) => format!("{}\\{}", self.ctx.namespace, n.text),
            None => "class@anonymous".to_string(),
        };
        let class = ClassScope { span: c.span, fqn, parent };
        self.out.classes.push(class.clone());
        self.doc_refs(c.doc);
        for n in c.extends.iter().chain(c.implements.iter()) {
            let f = self.ctx.resolve_class(&n.text);
            self.out.refs.push(Reference { span: n.span, target: RefTarget::Class(f) });
        }
        if let Some(b) = &c.backing {
            self.type_refs(b);
        }
        self.classes.push(class);
        c.attrs.iter().for_each(|a| self.visit_expr(a));
        for m in &c.members {
            self.visit_member(m);
        }
        self.classes.pop();
    }

    fn visit_function(&mut self, f: &'ast FunctionDecl) {
        f.attrs.iter().for_each(|a| self.visit_expr(a));
        let is_closure = f.name.is_none();
        let hint = if is_closure { self.closure_hint.take() } else { None };
        let mut scope = Scope { vars: FxHashMap::default(), span: f.span };
        if is_closure {
            self.closure_scope(f, &mut scope);
        }
        self.doc_refs(f.doc);
        self.declare_params(f, hint.as_ref(), &mut scope);
        if let Some(r) = &f.ret {
            self.type_refs(r);
        }
        self.scopes.push(scope);
        // arrow fns see the surrounding narrowings; other functions start clean
        let arrow = f.arrow_body.is_some();
        let saved = if arrow { None } else { Some(std::mem::take(&mut *self.narrowed.borrow_mut())) };
        let mark = self.narrowing_mark();
        if let Some(b) = &f.body {
            walk_stmts(self, b);
        }
        if let Some(e) = &f.arrow_body {
            self.visit_expr(e);
            let classes = self.infer(e);
            self.out.returns.push((e.span, classes));
        }
        self.restore_narrowing(mark);
        if let Some(s) = saved {
            *self.narrowed.borrow_mut() = s;
        }
        let done = self.scopes.pop().unwrap();
        self.snapshot(done);
    }

    fn visit_expr(&mut self, e: &'ast Expr) {
        if !self.want.is_empty() && self.want.contains(&(e.span.start, e.span.end)) {
            let classes = self.infer(e);
            self.out.expr_types.push((e.span, classes));
        }
        match &e.kind {
            ExprKind::Var(name) => self.visit_var(name, e.span),
            ExprKind::Name(n) => {
                if !matches!(n.text.to_ascii_lowercase().as_str(), "true" | "false" | "null") {
                    let c = self.ctx.resolve_const(&n.text);
                    self.out.refs.push(Reference { span: n.span, target: RefTarget::Const(c) });
                }
            }
            ExprKind::Call { callee, args } => self.visit_call(callee, args),
            ExprKind::New { class, args } => {
                self.class_ref_classes(class);
                args.iter().for_each(|a| self.visit_expr(&a.value));
            }
            ExprKind::MethodCall { obj, name, args, .. } => {
                self.visit_expr(obj);
                let receiver = self.infer(obj);
                self.visit_member_call(receiver, name, args, false);
            }
            ExprKind::Prop { obj, name, .. } => {
                self.visit_expr(obj);
                let receiver = self.infer(obj);
                self.visit_member_name(receiver, name, MemberKind::Property);
            }
            ExprKind::StaticCall { class, name, args } => {
                let receiver = self.class_ref_classes(class);
                self.visit_member_call(receiver, name, args, true);
            }
            ExprKind::StaticProp { class, name } => {
                let receiver = self.class_ref_classes(class);
                self.visit_member_name(receiver, name, MemberKind::Property);
            }
            ExprKind::ClassConst { class, name } => {
                let receiver = self.class_ref_classes(class);
                if name.text != "class" {
                    self.push_member_ref(name.span, MemberKind::Const, &name.text, receiver);
                }
            }
            ExprKind::InstanceOf { expr, class } => {
                self.visit_expr(expr);
                self.class_ref_classes(class);
            }
            ExprKind::Assign { target, value, op, .. } => self.visit_assign(target, value, op),
            ExprKind::Binary { op: op @ ("&&" | "and" | "||" | "or"), lhs, rhs } => {
                // `$x instanceof A && $x->foo()`: the right side sees what the left side implies
                self.visit_expr(lhs);
                let mark = self.narrowing_mark();
                self.narrow_by_condition(lhs, matches!(*op, "&&" | "and"));
                self.visit_expr(rhs);
                self.restore_narrowing(mark);
            }
            ExprKind::Ternary { cond, then, els } => {
                self.visit_expr(cond);
                let mark = self.narrowing_mark();
                self.narrow_by_condition(cond, true);
                if let Some(t) = then {
                    self.visit_expr(t);
                }
                self.restore_narrowing(mark);
                self.narrow_by_condition(cond, false);
                self.visit_expr(els);
                self.restore_narrowing(mark);
            }
            _ => walk_expr(self, e),
        }
    }
}

impl Analyzer<'_> {
    fn visit_use(&mut self, items: &[UseItem]) {
        for i in items {
            self.ctx.add_use(i.kind, &i.name, i.alias.as_deref());
            let target = match i.kind {
                UseKind::Class => RefTarget::Class(i.name.clone()),
                UseKind::Function => RefTarget::Function(vec![i.name.clone()]),
                UseKind::Const => RefTarget::Const(vec![i.name.clone()]),
            };
            self.out.refs.push(Reference { span: i.span, target });
        }
    }

    fn visit_try(&mut self, body: &[Stmt], catches: &[Catch], finally: Option<&[Stmt]>) {
        self.walk_branch(body);
        for c in catches {
            let mut classes = Vec::new();
            for t in &c.types {
                for cl in self.class_name_ref(t) {
                    push_unique(&mut classes, cl);
                }
            }
            if let Some(v) = &c.var {
                self.assign_var(&v.text, Span::new(v.span.start as usize, v.span.end as usize), classes, "=");
            }
            self.walk_branch(&c.body);
        }
        if let Some(f) = finally {
            walk_stmts(self, f);
        }
    }

    /// `foreach ($subject as $key => $value)`: `header` is `[subject, key?, value]`.
    fn visit_foreach(&mut self, header: &[Expr], blocks: &[Vec<Stmt>]) {
        let mut elems: Types = Vec::new();
        if let Some(subject) = header.first() {
            self.visit_expr(subject);
            elems = self.element_types_of_all(&self.infer(subject));
        }
        let last = header.len().saturating_sub(1);
        for (i, t) in header.iter().enumerate().skip(1) {
            match &t.kind {
                ExprKind::Var(n) if i == last => self.assign_var(n, t.span, elems.clone(), "="),
                _ if i == last => self.declare_targets(t, &elems),
                _ => self.declare_targets(t, &[]),
            }
        }
        let mark = self.narrowing_mark();
        for b in blocks {
            self.walk_branch(b);
        }
        self.restore_narrowing(mark);
    }

    fn visit_while(&mut self, header: &[Expr], blocks: &[Vec<Stmt>]) {
        header.iter().for_each(|h| self.visit_expr(h));
        let mark = self.narrowing_mark();
        header.iter().for_each(|h| self.narrow_by_condition(h, true));
        for b in blocks {
            self.walk_branch(b);
        }
        self.restore_narrowing(mark);
    }

    fn visit_member(&mut self, m: &Member) {
        match m {
            Member::Method { decl, .. } => self.visit_function(decl),
            Member::Property(p) => {
                self.doc_refs(p.doc);
                if let Some(t) = &p.ty {
                    self.type_refs(t);
                }
                if let Some(d) = &p.default {
                    self.visit_expr(d);
                }
            }
            Member::Const { items, .. } => {
                for i in items {
                    self.doc_refs(i.doc);
                    self.visit_expr(&i.value);
                }
            }
            Member::Case { value, .. } => {
                if let Some(v) = value {
                    self.visit_expr(v)
                }
            }
            Member::TraitUse { names, .. } => {
                for n in names {
                    let f = self.ctx.resolve_class(&n.text);
                    self.out.refs.push(Reference { span: n.span, target: RefTarget::Class(f) });
                }
            }
        }
    }

    /// Variables a closure starts with: all outer ones for `fn () =>`, the `use (...)` list otherwise.
    fn closure_scope(&mut self, f: &FunctionDecl, scope: &mut Scope) {
        let outer = self.scopes.last().unwrap();
        if f.arrow_body.is_some() {
            scope.vars = outer.vars.clone();
            return;
        }
        for u in &f.uses {
            self.out.refs.push(Reference {
                span: u.span,
                target: RefTarget::Variable { name: u.name.clone(), decl: Some(u.span), classes: Vec::new() },
            });
            let classes = match outer.vars.get(&u.name) {
                Some(v) => self.narrowed_type(&format!("${}", u.name)).unwrap_or_else(|| v.classes.clone()),
                None => Vec::new(),
            };
            scope.vars.insert(u.name.clone(), VarInfo { classes, decl: u.span });
        }
    }

    /// Parameter types come from the declaration, else `@param`, else the closure hint of the call.
    fn declare_params(&mut self, f: &FunctionDecl, hint: Option<&ParamTypes>, scope: &mut Scope) {
        let doc = f.doc.map(|d| docblock::parse(d.text(self.src)));
        for (pi, p) in f.params.iter().enumerate() {
            p.attrs.iter().for_each(|a| self.visit_expr(a));
            if let Some(t) = &p.ty {
                self.type_refs(t);
            }
            if let Some(d) = &p.default {
                self.visit_expr(d);
            }
            let mut classes = match &p.ty {
                Some(t) if !p.variadic => self.hint_classes(t),
                _ => Vec::new(),
            };
            if p.ty.is_none() && !p.variadic {
                if let Some(d) = &doc {
                    for tag in d.tags("param") {
                        let (ty, name) = docblock::parse_typed_var(&tag.body);
                        if name.as_deref() == Some(p.name.as_str()) {
                            classes = self.doc_classes(&ty);
                        }
                    }
                }
            }
            if classes.is_empty() && p.ty.is_none() && !p.variadic {
                if let Some(h) = hint.and_then(|h| h.get(pi)) {
                    classes = h.clone();
                }
            }
            self.out.refs.push(Reference {
                span: p.span,
                target: RefTarget::Variable { name: p.name.clone(), decl: Some(p.span), classes: classes.clone() },
            });
            scope.vars.insert(p.name.clone(), VarInfo { classes, decl: p.span });
        }
    }

    fn visit_var(&mut self, name: &str, span: Span) {
        if name == "this" {
            return;
        }
        let (decl, mut classes) = match self.lookup(name) {
            Some(v) => (Some(v.decl), v.classes.clone()),
            None => (None, Vec::new()),
        };
        if let Some(narrowed) = self.narrowed_type(&format!("${name}")) {
            classes = narrowed;
        }
        self.out.refs.push(Reference { span, target: RefTarget::Variable { name: name.to_string(), decl, classes } });
    }

    fn visit_call(&mut self, callee: &Expr, args: &[Arg]) {
        let mut hints = Vec::new();
        if let ExprKind::Name(n) = &callee.kind {
            let c = self.ctx.resolve_function(&n.text);
            self.out.refs.push(Reference { span: n.span, target: RefTarget::Function(c) });
            hints = self.function_hints(n.text.trim_start_matches('\\'), args);
        } else {
            self.visit_expr(callee);
        }
        self.visit_args(args, hints);
        // `assert($x instanceof User)`: `$x` is a User from here on
        if let ExprKind::Name(n) = &callee.kind {
            if n.text.eq_ignore_ascii_case("assert") {
                if let Some(c) = args.first() {
                    self.narrow_by_condition(&c.value, true);
                }
            }
        }
    }

    /// `$obj->name(...)` / `Cls::name(...)` with the receiver types already known.
    fn visit_member_call(&mut self, receiver: Types, name: &MemberName, args: &[Arg], is_static: bool) {
        let hints: ArgHints = match name {
            MemberName::Ident(id) => self.method_hints(&receiver, &id.text, args.len(), is_static),
            _ => Vec::new(),
        };
        self.visit_member_name(receiver, name, MemberKind::Method);
        self.visit_args(args, hints);
    }

    fn visit_member_name(&mut self, receiver: Types, name: &MemberName, kind: MemberKind) {
        match name {
            MemberName::Ident(id) => self.push_member_ref(id.span, kind, &id.text, receiver),
            MemberName::Expr(x) => self.visit_expr(x),
        }
    }

    fn visit_assign(&mut self, target: &Expr, value: &Expr, op: &str) {
        self.visit_expr(value);
        let classes = self.infer(value);
        match &target.kind {
            ExprKind::Var(n) => {
                // inside a branch the old types may still be there afterwards
                let op = if self.branch_depth > 0 && op == "=" { "|=" } else { op };
                self.assign_var(n, target.span, classes, op)
            }
            ExprKind::Array(_) => {
                self.pending_doc_type = None;
                self.declare_targets(target, &classes)
            }
            ExprKind::Index { base, .. } => {
                self.pending_doc_type = None;
                self.visit_expr(target);
                if let ExprKind::Var(n) = &base.kind {
                    let elems: Types = classes.iter().map(|c| format!("{c}[]")).collect();
                    self.add_var_types(n, target.span, elems);
                }
            }
            _ => {
                self.pending_doc_type = None;
                self.visit_expr(target);
                if let Some(p) = Self::expr_path(target) {
                    self.forget_narrowing(&p);
                }
            }
        }
    }
}
