//! What a function body does, collected in one pass: its `return`s, `yield`s and `throw`s, and
//! every way each variable gets a value. Nested functions and classes are not entered.

use super::{text, SuggestedType, TypeSuggester};
use crate::symbols::ParamInfo;
use php_syntax::walk::{walk_expr, walk_stmt, walk_stmts, Visitor};
use php_syntax::*;
use rustc_hash::FxHashMap;

/// One way a variable gets a value.
pub(super) enum Write<'a> {
    /// `$x = <expr>`
    Assign(&'a Expr),
    /// `$x .= <expr>`, `$x += <expr>`; `++` / `--` are `+=` without an expression.
    Compound(&'static str, Option<&'a Expr>),
    /// `$x[…] = …`: makes `$x` an array when it had no value (or `null`).
    Element,
    /// Passed to a by-reference parameter declared with this type.
    Typed(SuggestedType),
    /// `catch (E $x)`: an object; the analysis knows its classes.
    Object,
    /// foreach, list(), global, static, `=&`, an untyped by-reference parameter: anything.
    Unknown,
}

#[derive(Default)]
pub(super) struct Facts<'a> {
    pub params: Vec<&'a Param>,
    pub writes: FxHashMap<String, Vec<Write<'a>>>,
    /// Values of `return` statements; `None` for a bare `return;`.
    pub returns: Vec<Option<&'a Expr>>,
    pub yields: bool,
    /// `$$x`, `extract()`, `eval()`, `include`: any variable may change in ways not seen here.
    pub dynamic: bool,
    /// `throw new X` outside of `try` blocks, class names as written.
    pub thrown: Vec<String>,
}

impl<'a> Facts<'a> {
    pub fn param(&self, name: &str) -> Option<&'a Param> {
        self.params.iter().find(|p| p.name == name).copied()
    }
}

pub(super) fn collect<'a>(sg: &TypeSuggester<'_>, f: &'a FunctionDecl) -> Facts<'a> {
    let mut c = Collector { sg, facts: Facts::default(), in_try: 0 };
    c.facts.params = f.params.iter().collect();
    if let Some(body) = &f.body {
        walk_stmts(&mut c, body);
    }
    if let Some(e) = &f.arrow_body {
        c.visit_expr(e);
    }
    c.facts
}

struct Collector<'s, 'a> {
    sg: &'s TypeSuggester<'s>,
    facts: Facts<'a>,
    in_try: u32,
}

impl<'a> Collector<'_, 'a> {
    fn write(&mut self, name: &str, w: Write<'a>) {
        self.facts.writes.entry(name.trim_start_matches('$').to_string()).or_default().push(w);
    }

    /// Every variable inside an assignment target (`list($a, $b)`, `[$a, [$b]]`) gets any value.
    fn unknown_targets(&mut self, e: &'a Expr) {
        match &e.kind {
            ExprKind::Var(n) => self.write(n, Write::Unknown),
            ExprKind::Array(items) => items.iter().for_each(|i| self.unknown_targets(&i.value)),
            ExprKind::Assign { target, .. } => self.unknown_targets(target),
            ExprKind::Unary { op: "&", expr } => self.unknown_targets(expr),
            _ => {}
        }
    }

    fn assign(&mut self, target: &'a Expr, value: &'a Expr, op: &'static str, by_ref: bool) {
        match &target.kind {
            ExprKind::Var(n) if by_ref => self.write(n, Write::Unknown),
            ExprKind::Var(n) if op == "=" => self.write(n, Write::Assign(value)),
            ExprKind::Var(n) => self.write(n, Write::Compound(op, Some(value))),
            ExprKind::Index { .. } => {
                let mut base = target;
                while let ExprKind::Index { base: b, .. } = &base.kind {
                    base = b;
                }
                if let ExprKind::Var(n) = &base.kind {
                    self.write(n, Write::Element);
                }
            }
            ExprKind::Array(_) => self.unknown_targets(target),
            _ => {}
        }
    }

    /// Variables passed to by-reference parameters of the resolved callee.
    fn by_ref_args(&mut self, callee: Span, args: &'a [Arg]) {
        let Some(sym) = self.sg.resolved(callee).into_iter().next() else { return };
        for (i, a) in args.iter().enumerate() {
            let ExprKind::Var(n) = &a.value.kind else { continue };
            let param: Option<&ParamInfo> = match &a.name {
                Some(name) => sym.params.iter().find(|p| *p.name == *name.text),
                None => sym.params.get(i).or_else(|| sym.params.last().filter(|p| p.variadic)),
            };
            let Some(p) = param.filter(|p| p.by_ref) else { continue };
            let declared =
                p.ty.as_deref()
                    .filter(|_| p.ty_native)
                    .and_then(|t| text::type_text(t, &p.classes, sym.container.as_deref()))
                    .filter(|t| !t.need_classes && !t.ty.is_empty());
            self.write(n, declared.map(|t| Write::Typed(t.ty)).unwrap_or(Write::Unknown));
        }
    }
}

impl<'a> Visitor<'a> for Collector<'_, 'a> {
    fn visit_stmt(&mut self, s: &'a Stmt) {
        if let Some(value) = s.return_value(self.sg.src) {
            self.facts.returns.push(value);
        }
        match &s.kind {
            StmtKind::Control { kw: "foreach", header, .. } => {
                header.iter().skip(1).for_each(|t| self.unknown_targets(t));
            }
            StmtKind::Try { body, catches, finally } => {
                self.in_try += 1;
                walk_stmts(self, body);
                self.in_try -= 1;
                for c in catches {
                    if let Some(v) = &c.var {
                        self.write(&v.text, Write::Object);
                    }
                    walk_stmts(self, &c.body);
                }
                if let Some(f) = finally {
                    walk_stmts(self, f);
                }
                return;
            }
            StmtKind::Exprs(es) => {
                let head = s.span.text(self.sg.src).trim_start();
                let kw = head.split(|c: char| !c.is_ascii_alphabetic()).next().unwrap_or("");
                if kw.eq_ignore_ascii_case("global") || kw.eq_ignore_ascii_case("static") {
                    es.iter().for_each(|e| self.unknown_targets(e));
                }
            }
            _ => {}
        }
        walk_stmt(self, s);
    }

    fn visit_function(&mut self, f: &'a FunctionDecl) {
        // a closure may change variables it captures by reference; nested bodies are not entered
        for u in f.uses.iter().filter(|u| u.by_ref) {
            self.write(&u.name, Write::Unknown);
        }
    }

    fn visit_class(&mut self, _: &'a ClassDecl) {}

    fn visit_expr(&mut self, e: &'a Expr) {
        match &e.kind {
            ExprKind::Assign { target, value, op, by_ref } => self.assign(target, value, op, *by_ref),
            ExprKind::Unary { op: "++" | "--" | "p++" | "p--", expr } => {
                if let ExprKind::Var(n) = &expr.kind {
                    self.write(n, Write::Compound("+=", None));
                }
            }
            ExprKind::Yield { .. } | ExprKind::YieldFrom(_) => self.facts.yields = true,
            ExprKind::VarVar(_) | ExprKind::Include(_) => self.facts.dynamic = true,
            ExprKind::Call { callee, args } => {
                if let ExprKind::Name(n) = &callee.kind {
                    let name = n.text.trim_start_matches('\\');
                    if ["extract", "eval", "parse_str", "mb_parse_str"].iter().any(|f| name.eq_ignore_ascii_case(f))
                        && (args.len() < 2 || name.eq_ignore_ascii_case("extract"))
                    {
                        self.facts.dynamic = true;
                    }
                    self.by_ref_args(n.span, args);
                }
            }
            ExprKind::MethodCall { name: MemberName::Ident(id), args, .. }
            | ExprKind::StaticCall { name: MemberName::Ident(id), args, .. } => self.by_ref_args(id.span, args),
            ExprKind::Throw(x) if self.in_try == 0 => {
                if let ExprKind::New { class: ClassRef::Name(n), .. } = &x.kind {
                    if !self.facts.thrown.iter().any(|t| t.eq_ignore_ascii_case(&n.text)) {
                        self.facts.thrown.push(n.text.clone());
                    }
                }
            }
            _ => {}
        }
        walk_expr(self, e);
    }
}
