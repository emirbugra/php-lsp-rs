//! Code inspections: problems found by comparing a file's references with the index.

use crate::index::{strip_generic, Index, MemberKind};
use crate::symbols::SymbolKind;
use crate::{Analysis, RefTarget};
use php_syntax::{Parse, Span, Stmt, StmtKind};
use rustc_hash::FxHashMap;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Severity {
    Error,
    Warning,
    Hint,
}

#[derive(Clone, Debug)]
pub struct Problem {
    pub span: Span,
    /// Stable id used by quick fixes: `undefined-class`, `undefined-function`, `undefined-constant`,
    /// `undefined-method`, `undefined-class-constant`, `unused-import`, `unused-private`,
    /// `missing-abstract`, `deprecated`, `undefined-variable`, `missing-argument`, `too-many-arguments`, `type-mismatch`.
    pub code: &'static str,
    pub message: String,
    pub severity: Severity,
    /// Render greyed out.
    pub unnecessary: bool,
    /// Render struck through.
    pub deprecated: bool,
    pub args: Vec<String>,
}

pub struct Options {
    /// Index is complete enough to report unresolved names.
    pub unresolved: bool,
}

fn problem(span: Span, code: &'static str, severity: Severity, message: String, args: Vec<String>) -> Problem {
    Problem { span, code, message, severity, unnecessary: false, deprecated: false, args }
}

const MAGIC_CONSTS: &[&str] = &[
    "__DIR__",
    "__FILE__",
    "__LINE__",
    "__CLASS__",
    "__FUNCTION__",
    "__METHOD__",
    "__NAMESPACE__",
    "__TRAIT__",
    "__COMPILER_HALT_OFFSET__",
];
const NOT_FUNCTIONS: &[&str] = &["unset", "eval", "list", "array", "isset", "empty", "exit", "die", "__halt_compiler"];

pub fn inspect(
    index: &Index,
    uri: &str,
    src: &str,
    parse: &Parse,
    analysis: &Analysis,
    opts: &Options,
) -> Vec<Problem> {
    let mut out = crate::undefvars::undefined_variables(index, src, &parse.stmts, analysis);
    reference_problems(index, src, &parse.stmts, analysis, opts, &mut out);
    if !uri.ends_with(".phpstorm.meta.php") {
        crate::arg_count::argument_count_problems(index, src, &parse.stmts, analysis, &mut out);
        crate::type_check::type_problems(index, src, &parse.stmts, analysis, &mut out);
    }
    for u in unused_uses(src, &parse.stmts) {
        let mut p = problem(
            u.item_span,
            "unused-import",
            Severity::Hint,
            format!("Unused import '{}'", u.name),
            vec![u.name.clone()],
        );
        p.unnecessary = true;
        out.push(p);
    }
    declaration_problems(index, uri, src, &parse.stmts, opts, &mut out);
    out.sort_by_key(|p| (p.span.start, p.span.end));
    out
}

/// Names that resolve to nothing, and uses of deprecated declarations.
fn reference_problems(
    index: &Index,
    src: &str,
    stmts: &[Stmt],
    analysis: &Analysis,
    opts: &Options,
    out: &mut Vec<Problem>,
) {
    let mut use_stmts = Vec::new();
    top_level_uses(stmts, &mut use_stmts);
    let in_use = |sp: Span| use_stmts.iter().any(|u| u.span.start <= sp.start && sp.end <= u.span.end);
    for r in &analysis.refs {
        if matches!(r.target, RefTarget::Variable { .. }) {
            continue;
        }
        let locs = index.resolve(&r.target);
        if !locs.is_empty() {
            if locs.iter().any(|l| index.sym(*l).deprecated) {
                let name = php_name(&r.target);
                let mut p = problem(r.span, "deprecated", Severity::Hint, format!("'{name}' is deprecated"), vec![]);
                p.deprecated = true;
                out.push(p);
            }
            continue;
        }
        // `use Doctrine\ORM\Mapping as ORM;` imports a namespace, which is not in the class index
        let namespace_import = matches!(r.target, RefTarget::Class(_)) && in_use(r.span);
        if opts.unresolved && !namespace_import {
            out.extend(unresolved(index, r.span, r.span.text(src), &r.target));
        }
    }
}

/// The problem for a reference `written` in the source that resolves to no declaration.
fn unresolved(index: &Index, span: Span, written: &str, target: &RefTarget) -> Option<Problem> {
    let shown = written.trim_start_matches('\\');
    match target {
        RefTarget::Class(fqn) => Some(problem(
            span,
            "undefined-class",
            Severity::Warning,
            format!("Undefined class '{shown}'"),
            vec![fqn.clone(), written.to_string()],
        )),
        RefTarget::Function(c) if !NOT_FUNCTIONS.contains(&written.to_ascii_lowercase().as_str()) => Some(problem(
            span,
            "undefined-function",
            Severity::Warning,
            format!("Undefined function '{shown}'"),
            vec![c.first().cloned().unwrap_or_default(), written.to_string()],
        )),
        RefTarget::Const(c) if !MAGIC_CONSTS.contains(&written.to_ascii_uppercase().as_str()) => Some(problem(
            span,
            "undefined-constant",
            Severity::Warning,
            format!("Undefined constant '{shown}'"),
            vec![c.first().cloned().unwrap_or_default(), written.to_string()],
        )),
        RefTarget::Member { kind, name, classes } => unknown_member(index, span, *kind, name, classes),
        _ => None,
    }
}

/// `$x->foo()` / `X::FOO` where no receiver class has the member. Only reported when every
/// receiver class and its parents are known, and none of them can have members added at runtime.
fn unknown_member(index: &Index, span: Span, kind: MemberKind, name: &str, classes: &[String]) -> Option<Problem> {
    if kind == MemberKind::Property || classes.is_empty() {
        return None;
    }
    let known = classes.iter().all(|c| {
        let c = strip_generic(c);
        index.find_class(c).is_some() && index.hierarchy_complete(c)
    });
    if !known {
        return None;
    }
    if kind == MemberKind::Method && classes.iter().any(|c| index.has_dynamic_methods(c)) {
        return None;
    }
    // the runtime object is usually a subclass of an interface / abstract-typed receiver
    // trait methods call members supplied by the class that uses the trait
    if kind == MemberKind::Method
        && classes.iter().all(|c| {
            index
                .find_class(strip_generic(c))
                .map(|l| matches!(index.sym(l).kind, SymbolKind::Interface | SymbolKind::Trait))
                .unwrap_or(false)
        })
    {
        return None;
    }
    let cls = strip_generic(&classes[0]);
    let short = cls.rsplit('\\').next().unwrap_or(cls);
    Some(match kind {
        MemberKind::Method => problem(
            span,
            "undefined-method",
            Severity::Warning,
            format!("Method '{name}' not found in {short}"),
            vec![name.to_string(), cls.to_string()],
        ),
        _ => problem(
            span,
            "undefined-class-constant",
            Severity::Warning,
            format!("Class constant '{name}' not found in {short}"),
            vec![name.to_string(), cls.to_string()],
        ),
    })
}

/// Problems of the classes and members declared in this file.
fn declaration_problems(index: &Index, uri: &str, src: &str, stmts: &[Stmt], opts: &Options, out: &mut Vec<Problem>) {
    let Some(file) = index.file_by_uri(uri) else { return };
    let words = word_counts(src, stmts);
    for s in file.symbols.iter() {
        if opts.unresolved {
            out.extend(missing_abstract(index, s));
        }
        out.extend(unused_private(index, s, &words));
    }
}

/// A concrete class that does not implement all abstract methods it inherits.
fn missing_abstract(index: &Index, s: &crate::Symbol) -> Option<Problem> {
    if s.kind != SymbolKind::Class || s.is_abstract || s.name_span.is_empty() || !index.hierarchy_complete(&s.fqn) {
        return None;
    }
    let missing = index.missing_abstract(&s.fqn);
    if missing.is_empty() {
        return None;
    }
    let names: Vec<String> = missing.iter().map(|l| format!("{}()", index.sym(*l).name)).collect();
    Some(problem(
        s.name_span,
        "missing-abstract",
        Severity::Error,
        format!("Class '{}' must implement abstract method(s): {}", s.name, names.join(", ")),
        vec![s.fqn.to_string()],
    ))
}

/// A private method / property whose name appears only once in the file (its declaration).
fn unused_private(index: &Index, s: &crate::Symbol, words: &FxHashMap<String, u32>) -> Option<Problem> {
    if s.vis != php_syntax::Visibility::Private
        || s.is_virtual
        || !matches!(s.kind, SymbolKind::Method | SymbolKind::Property)
        || s.name.starts_with("__")
        || s.range == s.name_span
    {
        return None;
    }
    let owner_uses_traits = s
        .container
        .as_deref()
        .and_then(|c| index.find_class(c))
        .map(|l| !index.sym(l).traits().is_empty())
        .unwrap_or(false);
    let count = words.get(&s.name.to_ascii_lowercase()).copied().unwrap_or(0);
    if owner_uses_traits || count >= 2 {
        return None;
    }
    let what = if s.kind == SymbolKind::Method { "method" } else { "property" };
    let mut p = problem(
        s.name_span,
        "unused-private",
        Severity::Hint,
        format!("Private {what} '{}' is never used", s.name),
        vec![s.name.to_string()],
    );
    p.unnecessary = true;
    Some(p)
}

fn php_name(t: &RefTarget) -> String {
    match t {
        RefTarget::Class(f) => f.rsplit('\\').next().unwrap_or(f).to_string(),
        RefTarget::Function(c) | RefTarget::Const(c) => {
            let f = c.last().cloned().unwrap_or_default();
            f.rsplit('\\').next().unwrap_or(&f).to_string()
        }
        RefTarget::Member { name, .. } => name.clone(),
        RefTarget::Variable { name, .. } => name.clone(),
    }
}

// ---- imports ---------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct UnusedUse {
    pub name: String,
    /// The imported name including an optional `as Alias`.
    pub item_span: Span,
    pub stmt_span: Span,
    /// Every item of the statement is unused: delete the whole statement.
    pub whole_stmt: bool,
    /// Items in the statement and this item's position, for comma handling.
    pub item_index: usize,
    pub item_count: usize,
}

fn top_level_uses(stmts: &[Stmt], out: &mut Vec<Stmt>) {
    for s in stmts {
        match &s.kind {
            StmtKind::Use(_) => out.push(s.clone()),
            StmtKind::Namespace { body: Some(b), .. } => top_level_uses(b, out),
            _ => {}
        }
    }
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

/// Lower-cased word -> occurrence count, ignoring the `use` statements themselves.
fn word_counts(src: &str, stmts: &[Stmt]) -> FxHashMap<String, u32> {
    let mut uses = Vec::new();
    top_level_uses(stmts, &mut uses);
    let mut sorted: Vec<Span> = uses.iter().map(|s| s.span).collect();
    sorted.sort_by_key(|s| s.start);
    let mut counts: FxHashMap<String, u32> = FxHashMap::default();
    let b = src.as_bytes();
    let mut i = 0;
    let mut skip_i = 0;
    while i < b.len() {
        while skip_i < sorted.len() && (sorted[skip_i].end as usize) <= i {
            skip_i += 1;
        }
        if skip_i < sorted.len() && (sorted[skip_i].start as usize) <= i {
            i = sorted[skip_i].end as usize;
            continue;
        }
        if is_word_byte(b[i]) {
            let st = i;
            while i < b.len() && is_word_byte(b[i]) {
                i += 1;
            }
            let w = &src[st..i];
            if let Some(c) = counts.get_mut(&w.to_ascii_lowercase()) {
                *c += 1;
            } else {
                counts.insert(w.to_ascii_lowercase(), 1);
            }
        } else {
            i += 1;
        }
    }
    counts
}

pub fn unused_uses(src: &str, stmts: &[Stmt]) -> Vec<UnusedUse> {
    let words = word_counts(src, stmts);
    let mut uses = Vec::new();
    top_level_uses(stmts, &mut uses);
    let mut out = Vec::new();
    for s in &uses {
        let StmtKind::Use(items) = &s.kind else { continue };
        let unused: Vec<bool> = items
            .iter()
            .map(|it| {
                let key = it.alias.as_deref().unwrap_or_else(|| it.name.rsplit('\\').next().unwrap_or(&it.name));
                words.get(&key.to_ascii_lowercase()).copied().unwrap_or(0) == 0
            })
            .collect();
        let all = unused.iter().all(|u| *u);
        for (idx, it) in items.iter().enumerate() {
            if !unused[idx] {
                continue;
            }
            // extend over `as Alias`
            let mut end = it.span.end as usize;
            let rest = &src[end..];
            let trimmed = rest.trim_start();
            if trimmed.len() >= 3
                && trimmed[..2].eq_ignore_ascii_case("as")
                && trimmed.as_bytes()[2].is_ascii_whitespace()
            {
                let after = trimmed[2..].trim_start();
                let w: usize = after.bytes().take_while(|b| is_word_byte(*b)).count();
                end = src.len() - after.len() + w;
            }
            out.push(UnusedUse {
                name: it.name.clone(),
                item_span: Span::new(it.span.start as usize, end),
                stmt_span: s.span,
                whole_stmt: all,
                item_index: idx,
                item_count: items.len(),
            });
        }
    }
    out
}

// ---- call sites (for "create method") -----------------------------------------

#[derive(Debug, Clone)]
pub struct CallInfo {
    /// `$this->foo()`, `self::foo()`, `static::foo()`
    pub on_self: bool,
    pub is_static: bool,
    pub arg_names: Vec<String>,
}

pub fn find_call(stmts: &[Stmt], name_start: u32) -> Option<CallInfo> {
    use php_syntax::walk::*;
    use php_syntax::*;
    struct F {
        at: u32,
        found: Option<CallInfo>,
    }
    fn arg_names(args: &[Arg]) -> Vec<String> {
        let mut used: Vec<String> = Vec::new();
        args.iter()
            .enumerate()
            .map(|(i, a)| {
                let base = match &a.value.kind {
                    ExprKind::Var(n) => n.clone(),
                    ExprKind::Prop { name: MemberName::Ident(id), .. } => id.text.clone(),
                    _ => format!("arg{}", i + 1),
                };
                let mut n = base.clone();
                let mut k = 2;
                while used.contains(&n) {
                    n = format!("{base}{k}");
                    k += 1;
                }
                used.push(n.clone());
                n
            })
            .collect()
    }
    impl<'ast> Visitor<'ast> for F {
        fn visit_expr(&mut self, e: &'ast Expr) {
            match &e.kind {
                ExprKind::MethodCall { obj, name: MemberName::Ident(id), args, .. } if id.span.start == self.at => {
                    self.found = Some(CallInfo {
                        on_self: matches!(&obj.kind, ExprKind::Var(v) if v == "this"),
                        is_static: false,
                        arg_names: arg_names(args),
                    });
                }
                ExprKind::StaticCall { class, name: MemberName::Ident(id), args } if id.span.start == self.at => {
                    let on_self = matches!(class, ClassRef::Name(n)
                        if matches!(n.text.to_ascii_lowercase().as_str(), "self" | "static"));
                    self.found = Some(CallInfo { on_self, is_static: true, arg_names: arg_names(args) });
                }
                _ => {}
            }
            walk_expr(self, e);
        }
    }
    let mut f = F { at: name_start, found: None };
    walk_stmts(&mut f, stmts);
    f.found
}
