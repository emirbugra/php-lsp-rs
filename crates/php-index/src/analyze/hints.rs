//! Parameter types of closures passed to well-known functions and methods.
//!
//! In `$users->map(fn ($u) => $u->name)` the closure's `$u` has no declared type, but it is
//! clearly one element of `$users`. The functions here say, for each argument of a call, what
//! the parameters of a closure in that position receive.

use super::{Analyzer, ParamTypes, Types};
use crate::index::{generic_arg, strip_generic, ELOQUENT_BUILDER, ELOQUENT_COLLECTION};
use php_syntax::walk::Visitor;
use php_syntax::*;

/// Closure parameter types for each argument of a call (`None`: no hint for that argument).
pub(super) type ArgHints = Vec<Option<ParamTypes>>;

fn set(out: &mut ArgHints, i: usize, h: ParamTypes) {
    if i < out.len() {
        out[i] = Some(h);
    }
}

impl Analyzer<'_> {
    /// Visit call arguments; closure literals get parameter types from `hints[i]`.
    pub(super) fn visit_args(&mut self, args: &[Arg], hints: ArgHints) {
        for (i, a) in args.iter().enumerate() {
            if matches!(a.value.kind, ExprKind::Closure(_)) {
                self.closure_hint = hints.get(i).cloned().flatten();
            }
            self.visit_expr(&a.value);
            self.closure_hint = None;
        }
    }

    /// Parameter types of closures passed to a method of a collection / query builder.
    pub(super) fn method_hints(&self, receiver: &[String], name: &str, nargs: usize, is_static: bool) -> ArgHints {
        let mut out: ArgHints = vec![None; nargs];
        let l = name.to_ascii_lowercase();
        let elems = self.element_types_of_all(receiver);
        for r in receiver {
            let base = strip_generic(r);
            let builder = if base.eq_ignore_ascii_case(ELOQUENT_BUILDER) {
                generic_arg(r).map(|m| m.to_string())
            } else if is_static && self.index.is_model(base) {
                Some(base.to_string())
            } else {
                None
            };
            if let Some(m) = builder {
                let b = format!("{ELOQUENT_BUILDER}<{m}>");
                match l.as_str() {
                    "where" | "orwhere" | "tap" | "wherenot" | "orwherenot" => set(&mut out, 0, vec![vec![b]]),
                    "when" | "unless" => set(&mut out, 1, vec![vec![b]]),
                    "chunk" | "chunkbyid" | "chunkbyiddesc" => {
                        set(&mut out, 1, vec![vec![format!("{ELOQUENT_COLLECTION}<{m}>")]])
                    }
                    "each" => set(&mut out, 0, vec![vec![m]]),
                    _ => {}
                }
                return out;
            }
        }
        if elems.is_empty() {
            return out;
        }
        match l.as_str() {
            "map" | "each" | "filter" | "reject" | "first" | "last" | "contains" | "sortby" | "sortbydesc"
            | "groupby" | "keyby" | "flatmap" | "transform" | "mapwithkeys" | "partition" | "unique" | "every"
            | "some" | "search" | "takewhile" | "takeuntil" | "skipwhile" | "skipuntil" | "sum" | "avg" | "average"
            | "min" | "max" | "countby" | "sole" | "where" | "mapspread" | "mapinto" | "dump" | "firstor"
            | "containsstrict" | "doesntcontain" | "sortbykeys" | "uniquestrict" | "filter_map" | "lazy" => {
                set(&mut out, 0, vec![elems])
            }
            "reduce" => set(&mut out, 0, vec![Vec::new(), elems]),
            _ => {}
        }
        out
    }

    /// Parameter types of closures passed to built-in array functions and Laravel helpers.
    pub(super) fn function_hints(&self, name: &str, args: &[Arg]) -> ArgHints {
        let mut out: ArgHints = vec![None; args.len()];
        if name.contains('\\') {
            return out;
        }
        let elements_of_arg = |i: usize| -> Types {
            args.get(i).map(|a| self.element_types_of_all(&self.infer(&a.value))).unwrap_or_default()
        };
        match name.to_ascii_lowercase().as_str() {
            "array_map" => {
                let hints: ParamTypes = (1..args.len()).map(elements_of_arg).collect();
                set(&mut out, 0, hints);
            }
            "array_filter" | "array_walk" | "array_find" | "array_any" | "array_all" | "array_find_key" => {
                if out.len() > 1 {
                    set(&mut out, 1, vec![elements_of_arg(0)]);
                }
            }
            "tap" | "with" | "value" => {
                if out.len() > 1 {
                    set(&mut out, 1, vec![args.first().map(|a| self.infer(&a.value)).unwrap_or_default()]);
                }
            }
            "usort" | "uasort" => {
                if out.len() > 1 {
                    let e = elements_of_arg(0);
                    set(&mut out, 1, vec![e.clone(), e]);
                }
            }
            "array_reduce" if out.len() > 1 => set(&mut out, 1, vec![Vec::new(), elements_of_arg(0)]),
            _ => {}
        }
        out
    }
}
