//! Conditional return types: `@return ($key is null ? Repository : mixed)` picks the branch the
//! call's argument selects. When the argument's kind can't be told, both branches stay.

use super::Analyzer;
use crate::symbols::Symbol;
use crate::types::{self, CondArm, CondType, Types};
use php_syntax::*;

/// What a call argument is, as far as a conditional type's test cares.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum ArgKind {
    Null,
    Str,
    Int,
    Float,
    Bool(Option<bool>),
    Array,
    /// `Foo::class`, or a string literal naming a known class (other strings are `Str`).
    ClassString(Types),
    /// An object of one of these classes.
    Object(Types),
    Unknown,
}

impl Analyzer<'_> {
    /// Return atoms of `sym` for this call: the selected branch of a conditional type, else all.
    pub(super) fn return_atoms(&self, sym: &Symbol, args: &[Arg]) -> Types {
        if let Some(c) = &sym.cond_ret {
            if let Some(t) = self.eval_cond(sym, c, args, 0) {
                return t;
            }
        }
        sym.type_classes.iter().map(|c| c.to_string()).collect()
    }

    fn eval_cond(&self, sym: &Symbol, c: &CondType, args: &[Arg], depth: u32) -> Option<Types> {
        if depth > 8 {
            return None;
        }
        let kind = self.cond_arg_kind(sym, &c.param, args);
        let hit = self.cond_test(c, &kind)?;
        let arm = if hit != c.negated { &c.then } else { &c.els };
        Some(match arm {
            CondArm::Types(t) => t.clone(),
            CondArm::Cond(n) => self.eval_cond(sym, n, args, depth + 1).unwrap_or_else(|| arm.all_atoms()),
        })
    }

    /// The argument passed for parameter `param` (or its default value).
    fn cond_arg_kind(&self, sym: &Symbol, param: &str, args: &[Arg]) -> ArgKind {
        let Some(idx) = sym.params.iter().position(|p| &*p.name == param) else { return ArgKind::Unknown };
        let p = &sym.params[idx];
        if p.variadic {
            return ArgKind::Unknown;
        }
        let named = args.iter().find(|a| a.name.as_ref().map(|n| n.text == param).unwrap_or(false));
        let arg = named.or_else(|| {
            let before = args.iter().take(idx + 1);
            if before.clone().any(|a| a.spread) {
                return None;
            }
            args.get(idx).filter(|a| a.name.is_none())
        });
        match arg {
            Some(a) => self.expr_kind(&a.value),
            None if args.iter().any(|a| a.spread) => ArgKind::Unknown,
            None => match &p.default {
                Some(d) => literal_kind(d),
                None => ArgKind::Unknown,
            },
        }
    }

    pub(super) fn expr_kind(&self, e: &Expr) -> ArgKind {
        match &e.kind {
            ExprKind::Paren(x) => self.expr_kind(x),
            ExprKind::Lit => {
                if let Some(s) = e.string_value(self.src) {
                    let s = s.trim_start_matches('\\');
                    if !s.is_empty() && !s.contains([' ', '.', '/']) && self.index.find_class(s).is_some() {
                        return ArgKind::ClassString(vec![s.to_string()]);
                    }
                    return ArgKind::Str;
                }
                literal_kind(e.span.text(self.src))
            }
            ExprKind::Interp(_) => ArgKind::Str,
            ExprKind::Name(n) => literal_kind(&n.text),
            ExprKind::Array(_) => ArgKind::Array,
            ExprKind::Cast { ty, .. } => match *ty {
                "int" | "integer" => ArgKind::Int,
                "float" | "double" | "real" => ArgKind::Float,
                "string" | "binary" => ArgKind::Str,
                "bool" | "boolean" => ArgKind::Bool(None),
                "array" => ArgKind::Array,
                _ => ArgKind::Unknown,
            },
            ExprKind::Binary { op: ".", .. } => ArgKind::Str,
            ExprKind::Binary { op: "==" | "!=" | "===" | "!==" | "<" | ">" | "<=" | ">=" | "&&" | "||", .. }
            | ExprKind::Unary { op: "!", .. }
            | ExprKind::Isset(_)
            | ExprKind::Empty(_)
            | ExprKind::InstanceOf { .. } => ArgKind::Bool(None),
            ExprKind::Closure(_) => ArgKind::Object(vec!["Closure".to_string()]),
            ExprKind::ClassConst { name, .. } if name.text.eq_ignore_ascii_case("class") => {
                ArgKind::ClassString(self.only_classes(self.infer(e)))
            }
            ExprKind::New { .. } => {
                let t = self.infer(e);
                if t.is_empty() {
                    ArgKind::Unknown
                } else {
                    ArgKind::Object(t)
                }
            }
            ExprKind::Var(_) | ExprKind::Prop { .. } | ExprKind::MethodCall { .. } | ExprKind::Call { .. } => {
                // variables only count when they surely hold an object (class atoms, no arrays)
                let t = self.infer(e);
                if !t.is_empty() && t.iter().all(|a| !a.ends_with("[]") && !a.starts_with("class-string")) {
                    ArgKind::Object(t)
                } else {
                    ArgKind::Unknown
                }
            }
            _ => ArgKind::Unknown,
        }
    }

    /// Does an argument of `kind` pass the test of `c`? `None` when it can't be told.
    fn cond_test(&self, c: &CondType, kind: &ArgKind) -> Option<bool> {
        if *kind == ArgKind::Unknown {
            return None;
        }
        let test = c.test.trim_start_matches('?');
        let base = test.split('<').next().unwrap_or("").trim_start_matches('\\');
        let r = match base {
            "mixed" => true,
            "null" | "void" => *kind == ArgKind::Null,
            "string" | "non-empty-string" | "literal-string" | "callable-string" => {
                matches!(kind, ArgKind::Str | ArgKind::ClassString(_))
            }
            "class-string" | "interface-string" | "enum-string" => matches!(kind, ArgKind::ClassString(_)),
            "int" | "integer" | "positive-int" | "negative-int" | "non-negative-int" => *kind == ArgKind::Int,
            "float" | "double" => *kind == ArgKind::Float,
            "numeric" | "number" => matches!(kind, ArgKind::Int | ArgKind::Float),
            "bool" | "boolean" => matches!(kind, ArgKind::Bool(_)),
            "true" => match kind {
                ArgKind::Bool(Some(b)) => *b,
                ArgKind::Bool(None) => return None,
                _ => false,
            },
            "false" => match kind {
                ArgKind::Bool(Some(b)) => !*b,
                ArgKind::Bool(None) => return None,
                _ => false,
            },
            "array" | "list" | "non-empty-array" | "non-empty-list" | "iterable" => *kind == ArgKind::Array,
            "scalar" => matches!(kind, ArgKind::Str | ArgKind::Int | ArgKind::Float | ArgKind::Bool(_)),
            "object" => matches!(kind, ArgKind::Object(_)),
            "callable" => match kind {
                ArgKind::Object(t) if t.iter().any(|a| types::atom_base(a) == "Closure") => true,
                ArgKind::Str | ArgKind::Array | ArgKind::Object(_) => return None,
                _ => false,
            },
            _ => {
                // a class (or template) test
                let class = c.test_atoms.first().map(|a| types::atom_base(a).to_string());
                match (kind, class) {
                    (ArgKind::Object(t), Some(cls)) if !cls.starts_with('@') => {
                        let known: Vec<&String> =
                            t.iter().filter(|a| self.index.find_class(types::atom_base(a)).is_some()).collect();
                        if known.is_empty() {
                            return None;
                        }
                        let all = known.iter().all(|a| self.is_a(types::atom_base(a), &cls));
                        let none = known.iter().all(|a| !self.is_a(types::atom_base(a), &cls));
                        if all {
                            true
                        } else if none {
                            false
                        } else {
                            return None;
                        }
                    }
                    (ArgKind::Object(_), _) => return None,
                    (_, Some(cls)) if cls.starts_with('@') => return None,
                    (_, Some(_)) => false,
                    (_, None) => return None,
                }
            }
        };
        Some(r)
    }

    fn is_a(&self, class: &str, ancestor: &str) -> bool {
        class.eq_ignore_ascii_case(ancestor) || self.index.is_subclass_of(class, ancestor)
    }
}

/// Kind of a literal as written (`null`, `'x'`, `12`, `[]`, `true` …).
pub(super) fn literal_kind(text: &str) -> ArgKind {
    let t = text.trim();
    let l = t.trim_start_matches('\\').to_ascii_lowercase();
    match l.as_str() {
        "null" => return ArgKind::Null,
        "true" => return ArgKind::Bool(Some(true)),
        "false" => return ArgKind::Bool(Some(false)),
        _ => {}
    }
    if t.starts_with('\'') || t.starts_with('"') || t.starts_with("<<<") {
        return ArgKind::Str;
    }
    if t.starts_with('[') || l.starts_with("array(") {
        return ArgKind::Array;
    }
    let num = t.trim_start_matches('-').replace('_', "");
    if !num.is_empty() && num.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
        let is_hex = num.starts_with("0x") || num.starts_with("0X") || num.starts_with("0b") || num.starts_with("0B");
        if !is_hex && (num.contains('.') || num.contains(['e', 'E'])) {
            return ArgKind::Float;
        }
        return ArgKind::Int;
    }
    ArgKind::Unknown
}
