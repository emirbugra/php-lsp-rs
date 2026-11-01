//! The type of an expression (scalars included) and the return type of a function body.
//!
//! Scalars are worked out here from the syntax; object types come from the analysis, which
//! recorded the class types of every returned expression ([`crate::Analysis::returns`]).
//! Variables are typed flow-insensitively: the union of everything ever assigned to them.

use super::facts::{self, Facts, Write};
use super::{text, Atom, Partial, SuggestedType, TypeSuggester};
use crate::symbols::{Symbol, SymbolKind};
use crate::types;
use php_syntax::*;

impl TypeSuggester<'_> {
    /// Return type inferred from the body of `f`; None when any returned value is unknown.
    pub(super) fn infer_return(&self, f: &FunctionDecl) -> Option<SuggestedType> {
        let facts = facts::collect(self, f);
        if let Some(e) = &f.arrow_body {
            return self.returned(e, &facts);
        }
        let body = f.body.as_ref()?;
        if facts.yields {
            return Some(SuggestedType::of([Atom::Class("Generator".into())]));
        }
        let values: Vec<&Expr> = facts.returns.iter().flatten().copied().collect();
        if values.is_empty() {
            return Some(SuggestedType::of([Atom::Void]));
        }
        // `return;` next to `return $x;`, or falling off the end, is not allowed with a declared type
        if values.len() != facts.returns.len() || !always_returns(self.src, body) {
            return None;
        }
        let mut out = SuggestedType::default();
        for v in values {
            out.union(&self.returned(v, &facts)?);
        }
        Some(out)
    }

    /// Type of a returned expression: the scalar typing plus the classes the analysis recorded.
    fn returned(&self, e: &Expr, facts: &Facts) -> Option<SuggestedType> {
        let part = self.expr(e, facts, &mut Vec::new())?;
        let classes = self.analysis.returned_classes(e.span).map(|v| v.as_slice()).unwrap_or(&[]);
        let mut ty = part.ty;
        let mut found = false;
        for atom in classes {
            if let Some(elem) = types::array_elem(atom) {
                if ty.contains(&Atom::Array) && is_plain_class(elem) {
                    ty.add(Atom::ArrayOf(elem.to_string()));
                }
                continue;
            }
            if !part.need_classes {
                continue;
            }
            let base = types::atom_base(atom);
            if base.starts_with('@') {
                return None;
            }
            if base == "static" {
                ty.add(Atom::Static);
            } else if base.eq_ignore_ascii_case("class-string") {
                ty.add(Atom::String);
            } else if !types::is_non_class(base) {
                ty.add(Atom::Class(base.to_string()));
            } else {
                continue;
            }
            found = true;
        }
        if (part.need_classes && !found) || ty.is_empty() {
            return None;
        }
        Some(ty)
    }

    /// `guard`: variables being typed right now (a variable assigned from itself adds nothing).
    fn expr(&self, e: &Expr, facts: &Facts, guard: &mut Vec<String>) -> Option<Partial> {
        use Atom::*;
        let p = match &e.kind {
            ExprKind::Lit => Partial::of([text::literal(e.span.text(self.src))?]),
            ExprKind::Interp(_) => Partial::of([String]),
            ExprKind::Array(_) => Partial::of([Array]),
            ExprKind::Closure(_) => Partial::of([Class("Closure".into())]),
            ExprKind::Isset(_) | ExprKind::Empty(_) | ExprKind::InstanceOf { .. } => Partial::of([Bool]),
            ExprKind::Print(_) => Partial::of([Int]),
            ExprKind::Paren(x) | ExprKind::Clone(x) => self.expr(x, facts, guard)?,
            ExprKind::Var(name) => self.var(name, facts, guard)?,
            ExprKind::Name(n) => self.constant(n)?,
            ExprKind::Cast { ty, .. } => Partial::of([match *ty {
                "int" => Int,
                "float" => Float,
                "string" => String,
                "bool" => Bool,
                "array" => Array,
                "object" => Object,
                "unset" => Null,
                _ => return None,
            }]),
            ExprKind::Unary { op, expr } => self.unary(op, expr, facts, guard)?,
            ExprKind::Binary { op, lhs, rhs } => self.binary(op, lhs, rhs, facts, guard)?,
            ExprKind::Assign { value, op, by_ref: false, .. } if *op == "=" => self.expr(value, facts, guard)?,
            ExprKind::Assign { value, op, by_ref: false, .. } => self.compound(op, Some(value), facts, guard)?,
            ExprKind::Ternary { cond, then, els } => {
                let mut out = match then {
                    Some(t) => self.expr(t, facts, guard)?,
                    None => {
                        // `$a ?: $b` gives `$a` only when it is truthy
                        let mut c = self.expr(cond, facts, guard)?;
                        c.ty.remove(&Null);
                        c.ty.remove(&False);
                        if c.ty.contains(&Bool) {
                            c.ty.remove(&Bool);
                            c.ty.add(True);
                        }
                        c
                    }
                };
                out.merge(self.expr(els, facts, guard)?);
                out
            }
            ExprKind::Match { arms, .. } => {
                let mut out = Partial::default();
                for arm in arms {
                    out.merge(self.expr(&arm.body, facts, guard)?);
                }
                out
            }
            ExprKind::New { class, .. } => match class {
                ClassRef::Name(n) if n.text.eq_ignore_ascii_case("static") => Partial::of([Static]),
                ClassRef::Anon(_) => Partial::of([Object]),
                _ => Partial::objects(),
            },
            ExprKind::Call { callee, .. } => match &callee.kind {
                ExprKind::Name(n) => self.declared(n.span, false)?,
                _ => return None,
            },
            ExprKind::MethodCall { obj, name: MemberName::Ident(id), nullsafe, .. }
            | ExprKind::Prop { obj, name: MemberName::Ident(id), nullsafe, .. } => {
                let mut p = self.declared(id.span, false)?;
                // a `static` result is the receiver's class: only trust it when the receiver's type
                // is fully known, so the analysis cannot have missed one of its classes
                if p.need_classes {
                    self.expr(obj, facts, guard)?;
                }
                if *nullsafe {
                    p.ty.add(Null);
                }
                p
            }
            ExprKind::StaticCall { name: MemberName::Ident(id), .. }
            | ExprKind::StaticProp { name: MemberName::Ident(id), .. } => self.declared(id.span, false)?,
            ExprKind::ClassConst { name, .. } if name.text.eq_ignore_ascii_case("class") => Partial::of([String]),
            ExprKind::ClassConst { name, .. } => self.declared(name.span, true)?,
            _ => return None,
        };
        Some(p)
    }

    fn var(&self, name: &str, facts: &Facts, guard: &mut Vec<String>) -> Option<Partial> {
        if name == "this" {
            return Some(Partial::of([Atom::Static]));
        }
        if facts.dynamic {
            return None;
        }
        if guard.iter().any(|g| g == name) {
            return Some(Partial::default());
        }
        guard.push(name.to_string());
        let r = self.var_writes(name, facts, guard);
        guard.pop();
        r
    }

    fn var_writes(&self, name: &str, facts: &Facts, guard: &mut Vec<String>) -> Option<Partial> {
        let mut out = Partial::default();
        let mut any = false;
        let mut element = false;
        if let Some(p) = facts.param(name) {
            let hint = p.ty.as_ref()?;
            if p.variadic {
                out.ty.add(Atom::Array);
            } else {
                out.merge(self.hint(hint)?);
                if p.default.as_ref().is_some_and(|d| self.is_null(d)) {
                    out.ty.add(Atom::Null);
                }
            }
            any = true;
        }
        for w in facts.writes.get(name).map(|v| v.as_slice()).unwrap_or(&[]) {
            match w {
                Write::Assign(e) => out.merge(self.expr(e, facts, guard)?),
                Write::Compound(op, e) => out.merge(self.compound(op, *e, facts, guard)?),
                Write::Element => {
                    element = true;
                    continue;
                }
                Write::Typed(t) => out.ty.union(t),
                Write::Object => out.need_classes = true,
                Write::Unknown => return None,
            }
            any = true;
        }
        if element && (!any || out.ty.contains(&Atom::Null)) {
            out.ty.add(Atom::Array);
            any = true;
        }
        any.then_some(out)
    }

    /// A type written in this file's own signature (`int`, `?User`, `self`).
    fn hint(&self, hint: &TypeHint) -> Option<Partial> {
        let classes: Vec<String> = hint.names.iter().map(|n| self.analysis.ctx.resolve_class(&n.text)).collect();
        let container = self.class_at(hint.span.start).map(|c| c.fqn.as_str());
        text::type_text(&hint.text, &classes, container)
    }

    /// `true`, `false`, `null`, magic constants and constants with a literal value.
    fn constant(&self, n: &Name) -> Option<Partial> {
        let name = n.text.trim_start_matches('\\');
        let atom = match name.to_ascii_lowercase().as_str() {
            "true" => Atom::True,
            "false" => Atom::False,
            "null" => Atom::Null,
            "__line__" => Atom::Int,
            "__file__" | "__dir__" | "__function__" | "__class__" | "__method__" | "__namespace__" | "__trait__" => {
                Atom::String
            }
            _ => return self.declared(n.span, true),
        };
        Some(Partial::of([atom]))
    }

    /// Declared type of what the name at `span` resolves to: a function or method return type,
    /// a property type, or (`constant`) the type of a constant's literal value.
    fn declared(&self, span: Span, constant: bool) -> Option<Partial> {
        let syms = self.resolved(span);
        if syms.is_empty() {
            return None;
        }
        let mut out = Partial::default();
        for s in syms {
            out.merge(if constant { const_type(s)? } else { member_type(s)? });
        }
        Some(out)
    }

    /// Type of a default value or constant expression (`10`, `'x'`, `[]`, `null`, `self::LIMIT`).
    pub(super) fn constant_value(&self, e: &Expr) -> Option<SuggestedType> {
        let p = match &e.kind {
            ExprKind::Lit => Partial::of([text::literal(e.span.text(self.src))?]),
            ExprKind::Array(_) => Partial::of([Atom::Array]),
            ExprKind::Name(n) => self.constant(n)?,
            ExprKind::ClassConst { name, .. } if name.text.eq_ignore_ascii_case("class") => Partial::of([Atom::String]),
            ExprKind::ClassConst { name, .. } => self.declared(name.span, true)?,
            ExprKind::Unary { op: "-" | "+", expr } => Partial { ty: self.constant_value(expr)?, need_classes: false },
            ExprKind::Binary { op: ".", .. } => Partial::of([Atom::String]),
            _ => return None,
        };
        (!p.need_classes && !p.ty.is_empty()).then_some(p.ty)
    }

    fn unary(&self, op: &str, x: &Expr, facts: &Facts, guard: &mut Vec<String>) -> Option<Partial> {
        use Atom::*;
        Some(match op {
            "!" => Partial::of([Bool]),
            "~" => Partial::of([Int]),
            "@" | "&" => self.expr(x, facts, guard)?,
            _ => {
                // - + ++ --
                let t = self.expr(x, facts, guard)?;
                if t.need_classes || !t.ty.within(&[Int, Float]) {
                    return None;
                }
                t
            }
        })
    }

    fn binary(&self, op: &str, lhs: &Expr, rhs: &Expr, facts: &Facts, guard: &mut Vec<String>) -> Option<Partial> {
        use Atom::*;
        Some(match op {
            "." => Partial::of([String]),
            "==" | "!=" | "===" | "!==" | "<" | ">" | "<=" | ">=" | "&&" | "||" | "and" | "or" | "xor" => {
                Partial::of([Bool])
            }
            "<=>" => Partial::of([Int]),
            "<<" | ">>" => Partial::of([Int]),
            "??" => {
                let mut l = self.expr(lhs, facts, guard)?;
                l.ty.remove(&Null);
                l.merge(self.expr(rhs, facts, guard)?);
                l
            }
            "+" | "-" | "*" | "/" | "%" | "**" | "&" | "|" | "^" => {
                let l = self.expr(lhs, facts, guard)?;
                let r = self.expr(rhs, facts, guard)?;
                if l.need_classes || r.need_classes {
                    return None;
                }
                Partial { ty: arithmetic(op, &l.ty, &r.ty)?, need_classes: false }
            }
            _ => return None,
        })
    }

    /// `$x op= value` (`value` None for `++` / `--`).
    fn compound(&self, op: &str, value: Option<&Expr>, facts: &Facts, guard: &mut Vec<String>) -> Option<Partial> {
        use Atom::*;
        match op {
            ".=" => return Some(Partial::of([String])),
            "??=" => return self.expr(value?, facts, guard),
            "%=" | "<<=" | ">>=" => return Some(Partial::of([Int])),
            _ => {}
        }
        let rhs = match value {
            Some(v) => self.expr(v, facts, guard)?,
            None => Partial::of([Int]),
        };
        if rhs.need_classes {
            return None;
        }
        // the variable's own earlier values are covered by its other writes
        let bin = op.strip_suffix('=')?;
        Some(Partial { ty: arithmetic(bin, &SuggestedType::default(), &rhs.ty)?, need_classes: false })
    }
}

/// Result of `+ - * / % ** & | ^` on operands of types `a` and `b` (an empty side stands for
/// "a number like the other side").
fn arithmetic(op: &str, a: &SuggestedType, b: &SuggestedType) -> Option<SuggestedType> {
    use Atom::*;
    let mut all = a.clone();
    all.union(b);
    if all.is_empty() {
        return None;
    }
    let numbers = SuggestedType::of([Int, Float]);
    match op {
        "%" | "&" | "|" | "^" if all.within(&[Int, Float, Bool, True, False, Null]) => Some(SuggestedType::of([Int])),
        "&" | "|" | "^" if all.within(&[String]) => Some(SuggestedType::of([String])),
        "+" if all.atoms().iter().all(|x| matches!(x, Array | ArrayOf(_))) => Some(all),
        "+" | "-" | "*" if all.within(&[Int]) => Some(SuggestedType::of([Int])),
        "+" | "-" | "*" | "/" | "**" if all.within(&[Int, Float]) && (a.atoms() == [Float] || b.atoms() == [Float]) => {
            Some(SuggestedType::of([Float]))
        }
        "+" | "-" | "*" | "/" | "**" if all.within(&[Int, Float, String, Bool, True, False, Null]) => Some(numbers),
        _ => None,
    }
}

/// Declared return / property type of a symbol as seen from a caller.
fn member_type(s: &Symbol) -> Option<Partial> {
    if s.kind == SymbolKind::EnumCase {
        return Some(Partial::objects());
    }
    let mut p = text::type_text(s.ty.as_deref()?, &s.type_classes, s.container.as_deref())?;
    // `static` / `$this` / templates depend on the receiver: the analysis resolved them
    if p.ty.contains(&Atom::Static) {
        p.ty.remove(&Atom::Static);
        p.need_classes = true;
    }
    Some(p)
}

/// Type of a constant from the literal in its declaration (`const LIMIT = 10`).
fn const_type(s: &Symbol) -> Option<Partial> {
    if s.kind == SymbolKind::EnumCase {
        return Some(Partial::objects());
    }
    let value = s.detail.split_once(" = ")?.1.trim();
    let atom = match value.to_ascii_lowercase().as_str() {
        "true" => Atom::True,
        "false" => Atom::False,
        "null" => Atom::Null,
        v if v.starts_with('[') || v.starts_with("array(") || v.starts_with("array (") => Atom::Array,
        _ => text::literal(value)?,
    };
    Some(Partial::of([atom]))
}

fn is_plain_class(atom: &str) -> bool {
    !atom.contains(['<', '@', '[', '{']) && !types::is_non_class(atom) && atom != "static"
}

/// Does every path through `stmts` end in `return`, `throw` or `exit`?
fn always_returns(src: &str, stmts: &[Stmt]) -> bool {
    let Some(last) = stmts.iter().rev().find(|s| !matches!(s.kind, StmtKind::Nop)) else { return false };
    match &last.kind {
        StmtKind::Exprs(_) => last.jump_keyword(src) == Some("return"),
        StmtKind::Expr(e) => matches!(e.kind, ExprKind::Throw(_) | ExprKind::Exit(_)),
        StmtKind::Block(b) => always_returns(src, b),
        StmtKind::Control { kw: "if", header, blocks } => {
            blocks.len() > header.len() && blocks.iter().all(|b| always_returns(src, b))
        }
        StmtKind::Try { body, catches, finally } => {
            finally.as_ref().is_some_and(|f| always_returns(src, f))
                || (always_returns(src, body) && catches.iter().all(|c| always_returns(src, &c.body)))
        }
        _ => false,
    }
}
