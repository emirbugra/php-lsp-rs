//! Undefined-variable inspection.
//!
//! Deliberately conservative: only function / method / closure bodies are checked (global code is
//! often fed by includes), a variable counts as defined if it is written *anywhere* in the body, and
//! functions using `extract()`, `$$x`, `eval()` or `include` are skipped.

use crate::index::{Index, MemberKind};
use crate::inspections::{Problem, Severity};
use crate::{Analysis, RefTarget};
use php_syntax::walk::{walk_expr, walk_stmt, walk_stmts, Visitor};
use php_syntax::{Arg, ClassDecl, ClassRef, Expr, ExprKind, FunctionDecl, MemberName, Span, Stmt, StmtKind};
use rustc_hash::{FxHashMap, FxHashSet};

const SUPERGLOBALS: &[&str] = &[
    "this",
    "GLOBALS",
    "_GET",
    "_POST",
    "_COOKIE",
    "_FILES",
    "_SERVER",
    "_ENV",
    "_REQUEST",
    "_SESSION",
    "argv",
    "argc",
    "http_response_header",
    "php_errormsg",
];
const DYNAMIC_FUNCS: &[&str] = &["extract", "eval", "parse_str", "mb_parse_str", "import_request_variables"];

fn norm(n: &str) -> &str {
    n.trim_start_matches('$')
}

struct Ctx<'a> {
    index: &'a Index,
    src: &'a str,
    callee: FxHashMap<u32, &'a RefTarget>,
}

#[derive(Default)]
struct Body<'a> {
    defs: FxHashSet<String>,
    reads: Vec<(String, Span)>,
    dynamic: bool,
    suppress: u32,
    /// Nested function-likes found in this body (analysed afterwards).
    nested: Vec<&'a FunctionDecl>,
}

struct Collector<'a, 'c> {
    cx: &'c Ctx<'a>,
    body: Body<'a>,
}

impl<'a, 'c> Collector<'a, 'c> {
    fn define(&mut self, t: &'a Expr) {
        match &t.kind {
            ExprKind::Var(n) => {
                self.body.defs.insert(norm(n).to_string());
            }
            ExprKind::Index { base, index } => {
                self.define(base);
                if let Some(i) = index {
                    self.visit_expr(i);
                }
            }
            ExprKind::Array(items) => {
                for it in items {
                    if let Some(k) = &it.key {
                        self.visit_expr(k);
                    }
                    self.define(&it.value);
                }
            }
            ExprKind::Paren(x) => self.define(x),
            ExprKind::VarVar(_) => self.body.dynamic = true,
            _ => self.visit_expr(t),
        }
    }

    fn args(&mut self, callee_end: u32, args: &'a [Arg]) {
        let target = self.cx.callee.get(&callee_end).copied();
        let sym = target.and_then(|t| {
            let loc = match t {
                RefTarget::Class(fqn) => self.cx.index.find_member(fqn, "__construct", MemberKind::Method),
                t => self.cx.index.resolve(t).first().copied(),
            };
            loc.map(|l| self.cx.index.sym(l))
        });
        for (i, a) in args.iter().enumerate() {
            let plain = matches!(a.value.kind, ExprKind::Var(_) | ExprKind::Index { .. });
            if !plain {
                self.visit_expr(&a.value);
                continue;
            }
            let by_ref = match sym {
                // unresolved callee: assume it may take the argument by reference
                None => true,
                Some(s) => match s.params.get(i).or_else(|| s.params.last().filter(|p| p.variadic)) {
                    Some(p) => p.by_ref,
                    None => false,
                },
            };
            if by_ref {
                self.define(&a.value);
            } else {
                self.visit_expr(&a.value);
            }
        }
    }
}

/// `'a` is also the lifetime of the syntax tree, so nested functions can be kept for later.
impl<'a, 'c> Visitor<'a> for Collector<'a, 'c> {
    fn visit_stmt(&mut self, s: &'a Stmt) {
        match &s.kind {
            StmtKind::Exprs(es) => {
                let head = self.cx.src.get(s.span.start as usize..).unwrap_or("");
                let kw = head.split(|c: char| !c.is_ascii_alphabetic()).next().unwrap_or("").to_ascii_lowercase();
                if kw == "global" || kw == "static" {
                    for e in es {
                        self.define(e);
                    }
                    return;
                }
            }
            StmtKind::Control { kw: "foreach", header, blocks } => {
                if let Some(first) = header.first() {
                    self.visit_expr(first);
                }
                for t in header.iter().skip(1) {
                    self.define(t);
                }
                for b in blocks {
                    walk_stmts(self, b);
                }
                return;
            }
            StmtKind::Try { catches, .. } => {
                for c in catches {
                    if let Some(v) = &c.var {
                        self.body.defs.insert(norm(&v.text).to_string());
                    }
                }
            }
            StmtKind::Function(f) => {
                self.body.nested.push(f);
                return;
            }
            _ => {}
        }
        walk_stmt(self, s);
    }

    fn visit_function(&mut self, f: &'a FunctionDecl) {
        // closures: by-ref `use` writes to the outer variable, by-value reads it
        for u in &f.uses {
            if u.by_ref {
                self.body.defs.insert(norm(&u.name).to_string());
            } else if self.body.suppress == 0 {
                self.body.reads.push((norm(&u.name).to_string(), u.span));
            }
        }
        self.body.nested.push(f);
    }

    fn visit_class(&mut self, _c: &'a ClassDecl) {}

    fn visit_expr(&mut self, e: &'a Expr) {
        match &e.kind {
            ExprKind::Var(n) => {
                if self.body.suppress == 0 {
                    self.body.reads.push((norm(n).to_string(), e.span));
                }
            }
            ExprKind::VarVar(x) | ExprKind::Include(x) => {
                self.body.dynamic = true;
                self.visit_expr(x);
            }
            ExprKind::Assign { target, value, by_ref, .. } => {
                self.visit_expr(value);
                if *by_ref {
                    self.define(value);
                }
                self.define(target);
            }
            ExprKind::Unary { op, expr } if matches!(*op, "++" | "--" | "p++" | "p--") => self.define(expr),
            ExprKind::Unary { op: "@", expr } | ExprKind::Empty(expr) => self.quiet(|c| c.visit_expr(expr)),
            ExprKind::Isset(parts) => self.quiet(|c| parts.iter().for_each(|p| c.visit_expr(p))),
            ExprKind::Binary { op: "??", lhs, rhs } => {
                self.quiet(|c| c.visit_expr(lhs));
                self.visit_expr(rhs);
            }
            ExprKind::Call { callee, args } => self.call(callee, args),
            ExprKind::New { class, args } => match class {
                ClassRef::Name(n) => self.args(n.span.end, args),
                ClassRef::Expr(x) => {
                    self.visit_expr(x);
                    self.visit_args(args);
                }
                ClassRef::Anon(_) => self.visit_args(args),
            },
            ExprKind::MethodCall { obj, name, args, .. } => {
                self.visit_expr(obj);
                self.member_call(name, args);
            }
            ExprKind::StaticCall { class, name, args } => {
                if let ClassRef::Expr(x) = class {
                    self.visit_expr(x);
                }
                self.member_call(name, args);
            }
            _ => walk_expr(self, e),
        }
    }
}

impl<'a> Collector<'a, '_> {
    /// Reads inside `isset()`, `empty()`, `@`, `??` and `compact()` are not reported.
    fn quiet(&mut self, f: impl FnOnce(&mut Self)) {
        self.body.suppress += 1;
        f(self);
        self.body.suppress -= 1;
    }

    fn visit_args(&mut self, args: &'a [Arg]) {
        args.iter().for_each(|a| self.visit_expr(&a.value));
    }

    fn call(&mut self, callee: &'a Expr, args: &'a [Arg]) {
        let ExprKind::Name(n) = &callee.kind else {
            // `$fn($x)` / `(self::$f)($x)`: the callee is unknown, it may take `$x` by reference
            self.visit_expr(callee);
            self.args(u32::MAX, args);
            return;
        };
        let name = n.text.trim_start_matches('\\').to_ascii_lowercase();
        if DYNAMIC_FUNCS.contains(&name.as_str()) {
            self.body.dynamic = true;
        }
        if name == "compact" || name == "unset" {
            self.quiet(|c| c.visit_args(args));
        } else {
            self.args(n.span.end, args);
        }
    }

    /// `->name(args)` / `::name(args)`
    fn member_call(&mut self, name: &'a MemberName, args: &'a [Arg]) {
        match name {
            MemberName::Ident(i) => self.args(i.span.end, args),
            MemberName::Expr(x) => {
                self.visit_expr(x);
                self.visit_args(args);
            }
        }
    }
}

struct Finder<'a, 'c> {
    cx: &'c Ctx<'a>,
    out: Vec<Problem>,
}

impl<'a, 'c> Finder<'a, 'c> {
    fn analyse(&mut self, f: &'a FunctionDecl, inherited: Option<&FxHashSet<String>>) {
        let mut c = Collector { cx: self.cx, body: Body::default() };
        for p in &f.params {
            c.body.defs.insert(norm(&p.name).to_string());
        }
        for u in &f.uses {
            c.body.defs.insert(norm(&u.name).to_string());
        }
        if let Some(h) = inherited {
            c.body.defs.extend(h.iter().cloned());
        }
        if let Some(body) = &f.body {
            walk_stmts(&mut c, body);
        }
        if let Some(e) = &f.arrow_body {
            c.visit_expr(e);
        }
        let b = c.body;
        if !b.dynamic {
            for (name, span) in &b.reads {
                if b.defs.contains(name) || SUPERGLOBALS.contains(&name.as_str()) {
                    continue;
                }
                self.out.push(Problem {
                    span: *span,
                    code: "undefined-variable",
                    message: format!("Undefined variable '${name}'"),
                    severity: Severity::Warning,
                    unnecessary: false,
                    deprecated: false,
                    args: vec![name.clone()],
                });
            }
        }
        for n in b.nested {
            let inh = if n.arrow_body.is_some() { Some(&b.defs) } else { None };
            self.analyse(n, inh);
        }
    }
}

impl<'a, 'c> Visitor<'a> for Finder<'a, 'c> {
    fn visit_function(&mut self, f: &'a FunctionDecl) {
        self.analyse(f, None);
    }
}

pub fn undefined_variables<'a>(
    index: &'a Index,
    src: &'a str,
    stmts: &'a [Stmt],
    analysis: &'a Analysis,
) -> Vec<Problem> {
    let cx = Ctx { index, src, callee: analysis.refs.iter().map(|r| (r.span.end, &r.target)).collect() };
    let mut f = Finder { cx: &cx, out: Vec::new() };
    walk_stmts(&mut f, stmts);
    f.out
}
