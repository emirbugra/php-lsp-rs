//! Values that surely don't fit a native type: arguments (`foo('x')` for `foo(int $n)`) and
//! `return` values (`return null;` in a `: User` function). PHP throws a `TypeError` for these.
//!
//! Only plain cases are reported, so a warning is always right:
//! - the value's kind is plain to see: a literal, an array, `new X`, `X::class`, a cast, a string
//!   concatenation, or a variable that holds an object of a known class (checked against class
//!   types only, and only when the two classes can't be related through a subclass);
//! - the type is written in the signature (not a docblock) and every class in it is known;
//! - without `declare(strict_types=1)`, scalars that PHP converts (`'5'` for `int`, `1` for
//!   `string`) are fine;
//! - `null` for a built-in function's parameter is left alone (deprecated, not an error);
//! - intersection / DNF types, `static`, and generator functions are skipped.

use crate::arg_count::{callees, is_callable_syntax, Calls};
use crate::index::{strip_generic, Index, MemberKind};
use crate::inspections::{Problem, Severity};
use crate::origin::Origin;
use crate::symbols::ParamInfo;
use crate::{Analysis, RefTarget, Reference};
use php_syntax::walk::{walk_expr, walk_function, walk_stmts, Visitor};
use php_syntax::{ClassRef, Expr, ExprKind, FunctionDecl, Span, Stmt};
use rustc_hash::{FxHashMap, FxHashSet};

/// What a value is, as far as a type check cares.
#[derive(Debug, Clone, PartialEq)]
enum Kind {
    Null,
    Str,
    Int,
    Float,
    Bool(Option<bool>),
    Array,
    /// Objects of these classes. `exact`: the very class (`new X`), else a declared type whose
    /// value may be a subclass.
    Object(Vec<String>, bool),
}

impl Kind {
    fn describe(&self) -> String {
        match self {
            Kind::Null => "null".into(),
            Kind::Str => "string".into(),
            Kind::Int => "int".into(),
            Kind::Float => "float".into(),
            Kind::Bool(Some(true)) => "true".into(),
            Kind::Bool(Some(false)) => "false".into(),
            Kind::Bool(None) => "bool".into(),
            Kind::Array => "array".into(),
            Kind::Object(c, _) => {
                c.iter().map(|c| crate::refnames::last_segment(c).to_string()).collect::<Vec<_>>().join("|")
            }
        }
    }
}

/// A native type as written: `?int`, `string|array`, `User|null`.
#[derive(Clone)]
struct Expected<'a> {
    text: &'a str,
    /// Lowercased builtin parts (`int`, `null`, …).
    builtins: Vec<String>,
    /// Resolved class FQNs.
    classes: Vec<String>,
    nullable: bool,
}

impl<'a> Expected<'a> {
    /// `None` for types this check leaves alone. `classes` are the resolved class names of the type
    /// in written order (one per class part).
    fn new(text: &'a str, classes: Vec<String>, default_null: bool) -> Option<Self> {
        let t = text.trim();
        if t.contains(['&', '(']) || t.is_empty() {
            return None;
        }
        let mut builtins = Vec::new();
        let mut class_parts = 0;
        let mut nullable = t.starts_with('?') || default_null;
        for part in t.trim_start_matches('?').split('|') {
            let l = part.trim().trim_start_matches('\\').to_ascii_lowercase();
            match l.as_str() {
                "static" => return None,
                "self" | "parent" => class_parts += 1,
                "null" => {
                    nullable = true;
                    builtins.push(l);
                }
                "int" | "float" | "string" | "bool" | "true" | "false" | "array" | "iterable" | "callable"
                | "object" | "mixed" | "void" | "never" => builtins.push(l),
                _ => class_parts += 1,
            }
        }
        let classes: Vec<String> = classes.into_iter().map(|c| strip_generic(&c).to_string()).collect();
        if classes.len() != class_parts {
            return None;
        }
        Some(Expected { text: t, builtins, classes, nullable })
    }

    fn has(&self, b: &str) -> bool {
        self.builtins.iter().any(|x| x == b)
    }

    /// `Some(false)` when `kind` surely doesn't fit, `None` when it can't be told.
    fn accepts(&self, index: &Index, kind: &Kind, strict: bool) -> Option<bool> {
        if self.has("mixed") {
            return Some(true);
        }
        if self.has("void") || self.has("never") {
            return None;
        }
        let scalar = |s: &[&str]| s.iter().any(|b| self.has(b));
        let r = match kind {
            Kind::Null => self.nullable,
            Kind::Bool(v) => {
                let literal = match v {
                    Some(true) => self.has("true"),
                    Some(false) => self.has("false"),
                    // `(bool) $x` may be either one
                    None if self.has("true") || self.has("false") => return None,
                    None => false,
                };
                self.has("bool") || literal || (!strict && scalar(&["int", "float", "string"]))
            }
            Kind::Int => self.has("int") || self.has("float") || (!strict && scalar(&["string", "bool"])),
            Kind::Float => self.has("float") || (!strict && scalar(&["int", "string", "bool"])),
            Kind::Str => self.has("string") || self.has("callable") || (!strict && scalar(&["int", "float", "bool"])),
            Kind::Array => self.has("array") || self.has("iterable") || self.has("callable"),
            Kind::Object(classes, exact) => {
                if self.has("object") || self.has("callable") {
                    return Some(true);
                }
                if classes.iter().any(|c| index.find_class(c).is_none())
                    || (self.has("iterable") && index.find_class("Traversable").is_none())
                    || self.classes.iter().any(|c| index.find_class(c).is_none())
                {
                    return None;
                }
                // a variable's type may be narrowed where we don't follow (`is_array($v) ? … : …`)
                if !exact && self.builtins.iter().any(|b| b != "null") {
                    return None;
                }
                // a declared type may hold a subclass: only two unrelated classes surely don't fit
                let is_class = |c: &str| {
                    index.find_class(c).is_some_and(|l| index.sym(l).kind == crate::symbols::SymbolKind::Class)
                };
                if !exact
                    && classes.iter().any(|c| {
                        !is_class(c) || self.classes.iter().any(|p| !is_class(p) || index.is_subclass_of(p, c))
                    })
                {
                    return None;
                }
                let fits = |c: &str| {
                    self.classes.iter().any(|p| c.eq_ignore_ascii_case(p) || index.is_subclass_of(c, p))
                        || (self.has("iterable") && index.is_subclass_of(c, "Traversable"))
                        || (self.has("string")
                            && !strict
                            && (index.find_member(c, "__toString", MemberKind::Method).is_some()
                                || index.is_subclass_of(c, "Stringable")))
                };
                let n = classes.iter().filter(|c| fits(c)).count();
                if n == classes.len() {
                    true
                } else if n == 0 {
                    false
                } else {
                    return None;
                }
            }
        };
        Some(r)
    }
}

pub(crate) fn type_problems(index: &Index, src: &str, stmts: &[Stmt], analysis: &Analysis, out: &mut Vec<Problem>) {
    let strict = is_strict(src);
    let by_start: FxHashMap<u32, &Reference> = analysis.refs.iter().map(|r| (r.span.start, r)).collect();
    let by_end: FxHashMap<u32, &Reference> = analysis.refs.iter().map(|r| (r.span.end, r)).collect();
    let mut scan = Scan { src, typed_params: FxHashSet::default(), guarded: Vec::new() };
    walk_stmts(&mut scan, stmts);
    let ck =
        Checker { index, src, by_start: &by_start, strict, typed_params: scan.typed_params, guarded: scan.guarded };

    let mut calls = Calls::default();
    walk_stmts(&mut calls, stmts);
    for call in &calls.0 {
        if call.args.iter().any(|a| a.spread) || is_callable_syntax(src, call) || ck.is_guarded(call.span) {
            continue;
        }
        let Some(callee_ref) = by_end.get(&call.callee_end) else { continue };
        let locs = callees(index, &callee_ref.target);
        // a class declared twice (a library and a copy of it) may have either signature
        let doubled = |l: &crate::index::SymLoc| {
            index.sym(*l).container.as_deref().is_some_and(|c| index.find_classes(c).len() > 1)
        };
        if locs.is_empty() || locs.iter().any(|l| index.sym(*l).is_virtual || doubled(l)) {
            continue;
        }
        let name = index.sym(locs[0]).name.to_string();
        let mut positional = true;
        for (i, arg) in call.args.iter().enumerate() {
            if arg.name.is_some() {
                positional = false;
            }
            let Some(kind) = ck.kind(&arg.value) else { continue };
            // every declaration the call may reach must reject the value
            let mut rejected: Option<(usize, &ParamInfo)> = None;
            let mut all_reject = true;
            for l in &locs {
                let sym = index.sym(*l);
                let stub = index.origins().of(index.uri_of(*l)) == Origin::Stub;
                let param = match &arg.name {
                    Some(n) => sym.params.iter().enumerate().find(|(_, p)| *p.name == *n.text),
                    None if positional => sym
                        .params
                        .get(i)
                        .map(|p| (i, p))
                        .or_else(|| sym.params.last().filter(|p| p.variadic).map(|p| (sym.params.len() - 1, p))),
                    None => None,
                };
                let verdict = param.and_then(|(_, p)| {
                    if !p.ty_native || p.by_ref || (stub && kind == Kind::Null) {
                        return None;
                    }
                    let exp = Expected::new(
                        p.ty.as_deref()?,
                        p.classes.iter().map(|c| c.to_string()).collect(),
                        is_null_default(p),
                    )?;
                    exp.accepts(index, &kind, strict)
                });
                if verdict == Some(false) {
                    rejected = rejected.or(param);
                } else {
                    all_reject = false;
                    break;
                }
            }
            let (Some((pi, p)), true) = (rejected, all_reject) else { continue };
            let message = format!(
                "Argument #{} (${}) of '{name}' must be of type {}, {} given",
                pi + 1,
                p.name,
                p.ty.as_deref().unwrap_or(""),
                kind.describe()
            );
            out.push(problem(arg.value.span, message));
        }
    }

    let mut returns = Returns { ck: &ck, stack: Vec::new(), out };
    walk_stmts(&mut returns, stmts);
}

fn is_null_default(p: &ParamInfo) -> bool {
    p.default.as_deref().is_some_and(|d| d.trim().eq_ignore_ascii_case("null"))
}

fn problem(span: Span, message: String) -> Problem {
    Problem {
        span,
        code: "type-mismatch",
        message,
        severity: Severity::Error,
        unnecessary: false,
        deprecated: false,
        args: Vec::new(),
    }
}

/// `declare(strict_types=1)` at the top of the file.
fn is_strict(src: &str) -> bool {
    let head: String = src.chars().take(4096).filter(|c| !c.is_whitespace()).collect::<String>().to_ascii_lowercase();
    head.contains("declare(strict_types=1)")
}

struct Checker<'a> {
    index: &'a Index,
    src: &'a str,
    by_start: &'a FxHashMap<u32, &'a Reference>,
    strict: bool,
    /// Start offsets of parameters declared with a class type.
    typed_params: FxHashSet<u32>,
    /// Code under a feature / version check (`if (class_exists(X::class))`): written for another
    /// version of a library, so its calls may not fit the version the index holds.
    guarded: Vec<Span>,
}

impl Checker<'_> {
    fn is_guarded(&self, at: Span) -> bool {
        self.guarded.iter().any(|g| g.start <= at.start && at.end <= g.end)
    }
}

impl Checker<'_> {
    /// The kind of a value when it is plain to see.
    fn kind(&self, e: &Expr) -> Option<Kind> {
        match &e.kind {
            ExprKind::Paren(x) => self.kind(x),
            ExprKind::Lit => {
                let t = e.span.text(self.src).trim();
                if t.starts_with(['\'', '"']) || t.starts_with("<<<") {
                    return Some(Kind::Str);
                }
                number_kind(t)
            }
            ExprKind::Interp(_) => Some(Kind::Str),
            ExprKind::Name(n) => match n.text.trim_start_matches('\\').to_ascii_lowercase().as_str() {
                "null" => Some(Kind::Null),
                "true" => Some(Kind::Bool(Some(true))),
                "false" => Some(Kind::Bool(Some(false))),
                _ => None,
            },
            ExprKind::Array(_) => Some(Kind::Array),
            ExprKind::Cast { ty, .. } => match *ty {
                "int" | "integer" => Some(Kind::Int),
                "float" | "double" | "real" => Some(Kind::Float),
                "string" | "binary" => Some(Kind::Str),
                "bool" | "boolean" => Some(Kind::Bool(None)),
                "array" => Some(Kind::Array),
                _ => None,
            },
            ExprKind::Binary { op: ".", .. } => Some(Kind::Str),
            ExprKind::ClassConst { name, .. } if name.text.eq_ignore_ascii_case("class") => Some(Kind::Str),
            ExprKind::New { class: ClassRef::Name(n), .. } => match self.by_start.get(&n.span.start).map(|r| &r.target)
            {
                Some(RefTarget::Class(fqn)) if self.index.find_class(fqn).is_some() => {
                    Some(Kind::Object(vec![fqn.clone()], true))
                }
                _ => None,
            },
            ExprKind::Var(v) if v != "this" => {
                // only a parameter declared with a class type: other variables may hold more than
                // the types we know of (values from loops, untyped calls)
                let Some(RefTarget::Variable { classes, decl: Some(decl), .. }) =
                    self.by_start.get(&e.span.start).map(|r| &r.target)
                else {
                    return None;
                };
                if !self.typed_params.contains(&decl.start) {
                    return None;
                }
                // only plain classes the index knows; Eloquent magic types are left alone
                let ok = !classes.is_empty()
                    && classes.iter().all(|c| {
                        !c.contains(['<', '[', '@'])
                            && c != "static"
                            && !c.starts_with("Illuminate\\")
                            && self.index.find_class(c).is_some()
                            && !self.index.is_model(c)
                    });
                ok.then(|| Kind::Object(classes.clone(), false))
            }
            _ => None,
        }
    }

    /// The expected type of a function's native return type.
    fn return_type<'f>(&self, f: &'f FunctionDecl) -> Option<Expected<'f>> {
        let r = f.ret.as_ref()?;
        let l = r.text.to_ascii_lowercase();
        if ["generator", "iterable", "traversable", "iterator"].iter().any(|g| l.contains(g)) {
            return None;
        }
        let mut classes = Vec::new();
        for n in &r.names {
            let low = n.text.trim_start_matches('\\').to_ascii_lowercase();
            if crate::names::is_builtin_type(&low) && !matches!(low.as_str(), "self" | "parent") {
                continue;
            }
            match self.by_start.get(&n.span.start).map(|r| &r.target) {
                Some(RefTarget::Class(fqn)) => classes.push(fqn.clone()),
                _ => return None,
            }
        }
        Expected::new(&r.text, classes, false)
    }
}

fn number_kind(t: &str) -> Option<Kind> {
    let num = t.trim_start_matches('-').replace('_', "");
    if !num.chars().next()?.is_ascii_digit() {
        return None;
    }
    let lower = num.to_ascii_lowercase();
    if !(lower.starts_with("0x") || lower.starts_with("0b")) && (num.contains('.') || lower.contains('e')) {
        Some(Kind::Float)
    } else {
        Some(Kind::Int)
    }
}

struct Returns<'a, 'b> {
    ck: &'a Checker<'a>,
    /// Return type of each enclosing function (`None`: not checked).
    stack: Vec<Option<Expected<'a>>>,
    out: &'b mut Vec<Problem>,
}

impl<'ast: 'a, 'a, 'b> Visitor<'ast> for Returns<'a, 'b> {
    fn visit_function(&mut self, f: &'ast FunctionDecl) {
        let exp = if f.body.as_ref().is_some_and(|b| has_yield(b)) { None } else { self.ck.return_type(f) };
        if let (Some(body), Some(exp)) = (&f.arrow_body, &exp) {
            self.check(exp, body);
        }
        self.stack.push(exp);
        walk_function(self, f);
        self.stack.pop();
    }

    fn visit_stmt(&mut self, s: &'ast Stmt) {
        if let Some(Some(v)) = s.return_value(self.ck.src) {
            if let Some(Some(exp)) = self.stack.last().cloned() {
                self.check(&exp, v);
            }
        }
        php_syntax::walk::walk_stmt(self, s);
    }
}

impl Returns<'_, '_> {
    fn check(&mut self, exp: &Expected, e: &Expr) {
        if self.ck.is_guarded(e.span) {
            return;
        }
        let Some(kind) = self.ck.kind(e) else { return };
        if exp.accepts(self.ck.index, &kind, self.ck.strict) == Some(false) {
            let message = format!("Return value must be of type {}, {} returned", exp.text, kind.describe());
            self.out.push(problem(e.span, message));
        }
    }
}

/// Collects typed parameters and code under feature / version checks.
struct Scan<'s> {
    src: &'s str,
    typed_params: FxHashSet<u32>,
    guarded: Vec<Span>,
}

const GUARDS: &[&str] = &[
    "class_exists",
    "interface_exists",
    "enum_exists",
    "trait_exists",
    "method_exists",
    "function_exists",
    "property_exists",
    "version_compare",
    "defined(",
    "php_version",
    "::version",
    "instanceof",
    "is_callable",
    "is_a(",
    "is_subclass_of",
];

fn is_guard(text: &str) -> bool {
    let l = text.to_ascii_lowercase();
    GUARDS.iter().any(|g| l.contains(g))
}

impl<'ast> Visitor<'ast> for Scan<'_> {
    fn visit_function(&mut self, f: &'ast FunctionDecl) {
        for p in &f.params {
            if p.ty.as_ref().is_some_and(|t| !t.names.is_empty() && !t.text.contains(['&', '('])) {
                self.typed_params.insert(p.span.start);
            }
        }
        walk_function(self, f);
    }

    fn visit_stmt(&mut self, s: &'ast Stmt) {
        if let php_syntax::StmtKind::Control { header, .. } = &s.kind {
            if header.iter().any(|h| is_guard(h.span.text(self.src))) {
                self.guarded.push(s.span);
            }
        }
        php_syntax::walk::walk_stmt(self, s);
    }

    fn visit_expr(&mut self, e: &'ast Expr) {
        if let ExprKind::Ternary { cond, .. } = &e.kind {
            if is_guard(cond.span.text(self.src)) {
                self.guarded.push(e.span);
            }
        }
        walk_expr(self, e);
    }
}

/// The body yields (a generator), not counting nested functions.
fn has_yield(stmts: &[Stmt]) -> bool {
    struct Y(bool);
    impl<'ast> Visitor<'ast> for Y {
        fn visit_expr(&mut self, e: &'ast Expr) {
            if matches!(e.kind, ExprKind::Yield { .. } | ExprKind::YieldFrom(_)) {
                self.0 = true;
            }
            walk_expr(self, e);
        }
        fn visit_function(&mut self, _: &'ast FunctionDecl) {}
        fn visit_class(&mut self, _: &'ast php_syntax::ClassDecl) {}
    }
    let mut y = Y(false);
    walk_stmts(&mut y, stmts);
    y.0
}
