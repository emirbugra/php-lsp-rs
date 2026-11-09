//! Calls with the wrong number of arguments: a required parameter left out (PHP throws
//! `ArgumentCountError`), or more arguments than the callee declares.
//!
//! Only reported when every declaration the call may reach agrees. Left alone:
//! - calls with `...$spread` arguments, first-class callables `f(...)`;
//! - callees declared in docblocks (`@method`), whose parameter lists are often approximate;
//! - missing arguments of PHP's built-in functions: phpstorm-stubs writes some optional parameters
//!   without a default value (`array_keys($array, $filter_value, ...)`);
//! - extra arguments when the callee is variadic or reads `func_get_args()`;
//! - extra arguments to a method of an inferred receiver (`$user->save(1)`): PHP ignores extra
//!   arguments to user functions, so this is only worth a warning when the callee is certain
//!   (functions, `new X`, `X::`/`self::`/`static::`/`parent::`, `$this->`).
//! - `.phpstorm.meta.php` files, which are never run (they name methods as `X::f()`).

use crate::index::{Index, MemberKind, SymLoc};
use crate::inspections::{Problem, Severity};
use crate::origin::Origin;
use crate::symbols::Symbol;
use crate::{Analysis, RefTarget, Reference};
use php_syntax::walk::{walk_expr, walk_stmts, Visitor};
use php_syntax::{Arg, ClassRef, Expr, ExprKind, MemberName, Span, Stmt};
use rustc_hash::FxHashMap;

pub(crate) struct Call<'ast> {
    /// End of the callee name; equals the end of its [`crate::Reference`] span.
    pub(crate) callee_end: u32,
    pub(crate) span: Span,
    pub(crate) args: &'ast [Arg],
    /// The callee does not depend on an inferred type.
    pub(crate) exact: bool,
}

#[derive(Default)]
pub(crate) struct Calls<'ast>(pub(crate) Vec<Call<'ast>>);

impl<'ast> Visitor<'ast> for Calls<'ast> {
    fn visit_expr(&mut self, e: &'ast Expr) {
        let callee = match &e.kind {
            ExprKind::Call { callee, args } => match &callee.kind {
                ExprKind::Name(n) => Some((n.span.end, args, true)),
                _ => None,
            },
            ExprKind::MethodCall { obj, name: MemberName::Ident(i), args, .. } => {
                Some((i.span.end, args, matches!(&obj.kind, ExprKind::Var(v) if v == "this")))
            }
            ExprKind::StaticCall { class, name: MemberName::Ident(i), args } => {
                Some((i.span.end, args, matches!(class, ClassRef::Name(_))))
            }
            ExprKind::New { class: ClassRef::Name(n), args } => Some((n.span.end, args, true)),
            _ => None,
        };
        if let Some((end, args, exact)) = callee {
            self.0.push(Call { callee_end: end, span: e.span, args, exact });
        }
        walk_expr(self, e);
    }
}

enum Verdict {
    Ok,
    /// Names of the required parameters no argument reaches.
    Missing(Vec<String>),
    /// Declared parameter count; the extra arguments start at this index.
    TooMany(usize),
}

pub(crate) fn argument_count_problems(
    index: &Index,
    src: &str,
    stmts: &[Stmt],
    analysis: &Analysis,
    out: &mut Vec<Problem>,
) {
    let mut calls = Calls::default();
    walk_stmts(&mut calls, stmts);
    if calls.0.is_empty() {
        return;
    }
    let by_end: FxHashMap<u32, &Reference> = analysis.refs.iter().map(|r| (r.span.end, r)).collect();
    for call in &calls.0 {
        if call.args.iter().any(|a| a.spread) || is_callable_syntax(src, call) {
            continue;
        }
        let Some(callee_ref) = by_end.get(&call.callee_end) else { continue };
        let callees = callees(index, &callee_ref.target);
        if callees.is_empty() || callees.iter().any(|l| index.sym(*l).is_virtual) {
            continue;
        }
        let verdicts: Vec<Verdict> = callees.iter().map(|l| verdict(index, *l, call.args)).collect();
        let name = index.sym(callees[0]).name.to_string();
        if verdicts.iter().all(|v| matches!(v, Verdict::Missing(_))) {
            let Verdict::Missing(missing) = &verdicts[0] else { unreachable!() };
            let list = missing.iter().map(|n| format!("'${n}'")).collect::<Vec<_>>().join(", ");
            let message = if missing.len() == 1 {
                format!("Missing argument for parameter {list} of '{name}'")
            } else {
                format!("Missing arguments for parameters {list} of '{name}'")
            };
            out.push(problem(callee_ref.span, "missing-argument", Severity::Error, message));
        } else if call.exact && verdicts.iter().all(|v| matches!(v, Verdict::TooMany(_))) {
            let max = verdicts.iter().map(|v| if let Verdict::TooMany(n) = v { *n } else { 0 }).max().unwrap_or(0);
            let given = call.args.len();
            let (Some(first), Some(last)) = (call.args.get(max), call.args.last()) else { continue };
            let at = Span::new(first.value.span.start as usize, last.value.span.end as usize);
            let message = format!(
                "Too many arguments: '{name}' takes {max} argument{}, {given} given",
                if max == 1 { "" } else { "s" }
            );
            out.push(problem(at, "too-many-arguments", Severity::Warning, message));
        }
    }
}

fn problem(span: Span, code: &'static str, severity: Severity, message: String) -> Problem {
    Problem { span, code, message, severity, unnecessary: false, deprecated: false, args: Vec::new() }
}

/// `strlen(...)`: the parser keeps no argument for the `...`.
pub(crate) fn is_callable_syntax(src: &str, call: &Call) -> bool {
    if !call.args.is_empty() {
        return false;
    }
    let inner: String = src
        .get(call.callee_end as usize..call.span.end as usize)
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    inner == "(...)"
}

/// Every declaration the call may run.
pub(crate) fn callees(index: &Index, target: &RefTarget) -> Vec<SymLoc> {
    match target {
        RefTarget::Function(cands) => cands
            .iter()
            .map(|c| index.find_functions(c))
            .find(|l| !l.is_empty())
            .map(|l| l.to_vec())
            .unwrap_or_default(),
        RefTarget::Class(fqn) => index.find_member(fqn, "__construct", MemberKind::Method).into_iter().collect(),
        RefTarget::Member { kind: MemberKind::Method, .. } => index.resolve(target),
        _ => Vec::new(),
    }
}

fn verdict(index: &Index, loc: SymLoc, args: &[Arg]) -> Verdict {
    let sym: &Symbol = index.sym(loc);
    // from outside the class, a call to a private / protected method goes to `__call(Static)`
    if sym.vis != php_syntax::Visibility::Public && sym.container.as_ref().is_some_and(|c| index.has_dynamic_methods(c))
    {
        return Verdict::Ok;
    }
    let params = &sym.params;
    let positional = args.iter().take_while(|a| a.name.is_none()).count();
    let variadic = params.last().is_some_and(|p| p.variadic);
    if positional > params.len() && !variadic && !sym.reads_all_args {
        return Verdict::TooMany(params.len());
    }
    if index.origins().of(index.uri_of(loc)) == Origin::Stub {
        return Verdict::Ok;
    }
    let named = |n: &str| args.iter().filter_map(|a| a.name.as_ref()).any(|a| a.text == n);
    let missing: Vec<String> = params
        .iter()
        .enumerate()
        .filter(|(i, p)| *i >= positional && !p.variadic && p.default.is_none() && !named(&p.name))
        .map(|(_, p)| p.name.to_string())
        .collect();
    if missing.is_empty() {
        Verdict::Ok
    } else {
        Verdict::Missing(missing)
    }
}
