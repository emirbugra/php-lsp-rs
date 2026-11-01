//! Type suggestions for the "add return type", "add type" and "add PHPDoc" code actions.
//!
//! A suggestion must never be narrower than what the code really does: a declared type that is too
//! narrow throws a `TypeError` at runtime. So every step gives up (returns `None`) as soon as one
//! part of the answer is unknown.
//!
//! - `text.rs`: type text (`?int`, `User[]`, `array<string, int>`) and literals -> [`Atom`]s
//! - `facts.rs`: what a function body does: its `return`s, `yield`s, `throw`s and variable writes
//! - `expr.rs`: the type of an expression, scalars included, and the return type of a function
//! - `inherit.rs`: types that parent classes force and subclasses forbid

mod expr;
mod facts;
mod inherit;
mod text;

use crate::analyze::{Analysis, ClassScope};
use crate::index::Index;
use crate::symbols::{Symbol, SymbolKind};
use php_syntax::{Expr, FunctionDecl, PropertyDecl, Span, Visibility};
use rustc_hash::FxHashMap;

/// A PHP language version (`major.minor`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PhpVersion(pub u8, pub u8);

impl PhpVersion {
    pub const LATEST: PhpVersion = PhpVersion(8, 4);

    /// `8.1`, `7.4.33`, `8`
    pub fn parse(s: &str) -> Option<PhpVersion> {
        let mut parts = s.trim().split('.');
        let major = parts.next()?.trim().parse().ok()?;
        let minor = match parts.next() {
            Some(m) if !m.trim().is_empty() => m.trim().parse().ok()?,
            _ => 0,
        };
        Some(PhpVersion(major, minor))
    }

    /// Lowest version a composer constraint allows: `^7.4|^8.0` -> 7.4, `>=8.1 <8.4` -> 8.1, `8.*` -> 8.0.
    pub fn from_constraint(constraint: &str) -> Option<PhpVersion> {
        constraint
            .split('|')
            .filter(|alt| !alt.trim().is_empty())
            .filter_map(|alt| alt.split([' ', ',']).filter(|c| !c.is_empty()).filter_map(lower_bound).max())
            .min()
    }
}

/// The version a single constraint (`^7.4`, `>=8.1`, `~8.2.0`, `8.*`) starts at; None for upper bounds.
fn lower_bound(c: &str) -> Option<PhpVersion> {
    if c.starts_with('<') || c.starts_with("!=") {
        return None;
    }
    let v = c.trim_start_matches(['^', '~', '>', '=', 'v']);
    let v = v.split(['-', '@']).next()?.trim_end_matches('*').trim_end_matches('.');
    PhpVersion::parse(v)
}

/// One member of a union type. Atoms are written in the order of the variants.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Atom {
    SelfType,
    Static,
    /// A class-like, fully qualified without the leading backslash.
    Class(String),
    /// An array of this class: `User[]` in a docblock, `array` in a signature.
    ArrayOf(String),
    Array,
    Iterable,
    Callable,
    Object,
    Int,
    Float,
    String,
    Bool,
    True,
    False,
    Mixed,
    Void,
    Never,
    Null,
}

/// Where a type is written: what PHP accepts differs a little between them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Param,
    Return,
    Property,
}

/// A union of [`Atom`]s, kept sorted and without duplicates. Empty means "nothing known yet".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SuggestedType {
    atoms: Vec<Atom>,
}

impl SuggestedType {
    pub fn of(atoms: impl IntoIterator<Item = Atom>) -> Self {
        let mut t = SuggestedType::default();
        atoms.into_iter().for_each(|a| t.add(a));
        t
    }

    pub fn atoms(&self) -> &[Atom] {
        &self.atoms
    }

    pub fn is_empty(&self) -> bool {
        self.atoms.is_empty()
    }

    pub fn contains(&self, a: &Atom) -> bool {
        self.atoms.iter().any(|x| same_atom(x, a))
    }

    pub fn add(&mut self, a: Atom) {
        if !self.contains(&a) {
            self.atoms.push(a);
            self.atoms.sort();
        }
    }

    pub fn union(&mut self, other: &SuggestedType) {
        other.atoms.iter().for_each(|a| self.add(a.clone()));
    }

    pub fn remove(&mut self, a: &Atom) {
        self.atoms.retain(|x| !same_atom(x, a));
    }

    /// Every atom is one of `allowed`.
    fn within(&self, allowed: &[Atom]) -> bool {
        self.atoms.iter().all(|a| allowed.contains(a))
    }

    /// `true|false` -> `bool`; `mixed` swallows everything else.
    fn simplified(&self) -> Vec<Atom> {
        if self.atoms.contains(&Atom::Mixed) {
            return vec![Atom::Mixed];
        }
        let mut v = self.atoms.clone();
        if v.contains(&Atom::Bool) || (v.contains(&Atom::True) && v.contains(&Atom::False)) {
            v.retain(|a| !matches!(a, Atom::True | Atom::False | Atom::Bool));
            v.push(Atom::Bool);
            v.sort();
        }
        v
    }

    /// The type as PHP code for `place`, or None when `version` cannot express it.
    /// `name` turns a class FQN into the text to write (short name, alias or `\Fqn`).
    pub fn native(&self, version: PhpVersion, place: Place, name: &dyn Fn(&str) -> String) -> Option<String> {
        let since = |major, minor| version >= PhpVersion(major, minor);
        match place {
            Place::Return if !since(7, 0) => return None,
            Place::Property if !since(7, 4) => return None,
            _ => {}
        }
        let mut atoms: Vec<Atom> = Vec::new();
        for a in self.simplified() {
            let a = if matches!(a, Atom::ArrayOf(_)) { Atom::Array } else { a };
            if !atoms.contains(&a) {
                atoms.push(a);
            }
        }
        atoms.sort();
        match atoms.as_slice() {
            [] => return None,
            [Atom::Mixed] => return since(8, 0).then(|| "mixed".to_string()),
            [Atom::Void] => return (place == Place::Return && since(7, 1)).then(|| "void".to_string()),
            [Atom::Never] => return (place == Place::Return && since(8, 1)).then(|| "never".to_string()),
            _ if atoms.iter().any(|a| matches!(a, Atom::Void | Atom::Never | Atom::Mixed)) => return None,
            _ => {}
        }
        let nullable = atoms.contains(&Atom::Null);
        atoms.retain(|a| *a != Atom::Null);
        if atoms.is_empty() {
            return None;
        }
        let union = atoms.len() > 1;
        let allowed = |a: &Atom| match a {
            Atom::Int | Atom::Float | Atom::String | Atom::Bool => since(7, 0),
            Atom::Array | Atom::Class(_) | Atom::SelfType => true,
            Atom::Callable => place != Place::Property,
            Atom::Iterable => since(7, 1),
            Atom::Object => since(7, 2),
            Atom::Static => place == Place::Return && since(8, 0),
            Atom::False => since(8, 2) || (since(8, 0) && union),
            Atom::True => since(8, 2),
            _ => false,
        };
        if !atoms.iter().all(allowed) {
            return None;
        }
        let parts: Vec<String> = atoms.iter().map(|a| render(a, name)).collect();
        if !union {
            return match (nullable, &atoms[0]) {
                (false, _) => Some(parts[0].clone()),
                (true, Atom::True | Atom::False) => since(8, 2).then(|| format!("{}|null", parts[0])),
                (true, _) => since(7, 1).then(|| format!("?{}", parts[0])),
            };
        }
        if !since(8, 0) {
            return None;
        }
        let mut s = parts.join("|");
        if nullable {
            s.push_str("|null");
        }
        Some(s)
    }

    /// The type for a docblock: like [`native`](Self::native) but with `User[]` and no version limits.
    pub fn doc(&self, name: &dyn Fn(&str) -> String) -> String {
        let atoms = self.simplified();
        if atoms.is_empty() {
            return "mixed".to_string();
        }
        let typed_array = atoms.iter().any(|a| matches!(a, Atom::ArrayOf(_)));
        atoms
            .iter()
            .filter(|a| !(typed_array && **a == Atom::Array))
            .map(|a| render(a, name))
            .collect::<Vec<_>>()
            .join("|")
    }
}

fn same_atom(a: &Atom, b: &Atom) -> bool {
    match (a, b) {
        (Atom::Class(x), Atom::Class(y)) | (Atom::ArrayOf(x), Atom::ArrayOf(y)) => x.eq_ignore_ascii_case(y),
        _ => a == b,
    }
}

fn render(a: &Atom, name: &dyn Fn(&str) -> String) -> String {
    match a {
        Atom::SelfType => "self".into(),
        Atom::Static => "static".into(),
        Atom::Class(c) => name(c),
        Atom::ArrayOf(c) => format!("{}[]", name(c)),
        Atom::Array => "array".into(),
        Atom::Iterable => "iterable".into(),
        Atom::Callable => "callable".into(),
        Atom::Object => "object".into(),
        Atom::String => "string".into(),
        Atom::Int => "int".into(),
        Atom::Float => "float".into(),
        Atom::Bool => "bool".into(),
        Atom::True => "true".into(),
        Atom::False => "false".into(),
        Atom::Mixed => "mixed".into(),
        Atom::Void => "void".into(),
        Atom::Never => "never".into(),
        Atom::Null => "null".into(),
    }
}

/// A type worked out for part of an expression. `need_classes`: some part is an object whose
/// classes only the analysis knows (`new Foo`, a `static` return, a template); they are added
/// from [`Analysis::returns`] at the end.
#[derive(Clone, Debug, Default)]
struct Partial {
    ty: SuggestedType,
    need_classes: bool,
}

impl Partial {
    fn of(atoms: impl IntoIterator<Item = Atom>) -> Partial {
        Partial { ty: SuggestedType::of(atoms), need_classes: false }
    }

    fn objects() -> Partial {
        Partial { ty: SuggestedType::default(), need_classes: true }
    }

    fn merge(&mut self, other: Partial) {
        self.ty.union(&other.ty);
        self.need_classes |= other.need_classes;
    }
}

/// Suggested type of a property, and whether ` = null` must be added because the property had no
/// default (an untyped property starts as `null`, a typed one would start uninitialized).
#[derive(Clone, Debug, PartialEq)]
pub struct PropertyType {
    pub ty: SuggestedType,
    pub add_null_default: bool,
}

/// Works out types for declarations of one analysed file.
pub struct TypeSuggester<'a> {
    pub index: &'a Index,
    pub analysis: &'a Analysis,
    pub src: &'a str,
    pub uri: &'a str,
    /// Reference index by span, for looking up what a call or constant resolves to.
    refs: FxHashMap<(u32, u32), usize>,
}

impl<'a> TypeSuggester<'a> {
    pub fn new(index: &'a Index, analysis: &'a Analysis, src: &'a str, uri: &'a str) -> Self {
        let refs = analysis.refs.iter().enumerate().map(|(i, r)| ((r.span.start, r.span.end), i)).collect();
        TypeSuggester { index, analysis, src, uri, refs }
    }

    /// Declarations the name at `span` resolves to. For functions and constants every definition
    /// counts: `function_exists()` guards let several packages define the same one, and which of
    /// them PHP loads is not known here.
    fn resolved(&self, span: Span) -> Vec<&'a Symbol> {
        let Some(&i) = self.refs.get(&(span.start, span.end)) else { return Vec::new() };
        let mut locs = self.index.resolve(&self.analysis.refs[i].target);
        if let Some(&first) = locs.first() {
            let s = self.index.sym(first);
            let all = match s.kind {
                SymbolKind::Function => self.index.find_functions(&s.fqn),
                SymbolKind::Const => self.index.find_consts(&s.fqn),
                _ => &[],
            };
            if all.len() > 1 {
                locs = all.to_vec();
            }
        }
        locs.into_iter().map(|l| self.index.sym(l)).collect()
    }

    /// The indexed symbol declared with its name at `name_span` in this file.
    fn own_symbol(&self, name_span: Span, kinds: &[SymbolKind]) -> Option<&'a Symbol> {
        let file = self.index.file_by_uri(self.uri)?;
        file.symbols.iter().find(|s| s.name_span == name_span && kinds.contains(&s.kind))
    }

    fn class_at(&self, offset: u32) -> Option<&'a ClassScope> {
        self.analysis.class_at(offset as usize)
    }

    /// Return type for the function, method, closure or arrow function `f`.
    pub fn return_type(&self, f: &FunctionDecl) -> Option<SuggestedType> {
        if f.ret.is_some() {
            return None;
        }
        let own = f.name.as_ref().and_then(|n| self.own_symbol(n.span, &[SymbolKind::Function, SymbolKind::Method]));
        let method = own.filter(|s| s.kind == SymbolKind::Method);
        if let Some(m) = method {
            if ["__construct", "__destruct"].iter().any(|n| m.name.eq_ignore_ascii_case(n)) {
                return None;
            }
        }
        let mut ty = self.infer_return(f).or_else(|| own.and_then(|s| self.declared_by_doc(s))).map(widen_bool);
        let (Some(m), Some(class)) = (method, self.class_at(f.span.start)) else { return self.existing(ty?) };
        match inherit::parent_return(self.index, &class.fqn, &m.name) {
            inherit::Inherited::Typed(parent) => {
                if !ty.as_ref().is_some_and(|t| inherit::subtype(self.index, t, &parent, &class.fqn)) {
                    ty = Some(parent);
                }
            }
            inherit::Inherited::Untyped | inherit::Inherited::Nothing => {}
        }
        let ty = self.existing(ty?)?;
        if m.vis != Visibility::Private && !inherit::children_accept_return(self.index, &class.fqn, &m.name, &ty) {
            return None;
        }
        Some(ty)
    }

    /// Return type of `f` judged from its body (or `@return`) alone, without checking parent
    /// classes and subclasses: good enough for a docblock.
    pub fn inferred_return(&self, f: &FunctionDecl) -> Option<SuggestedType> {
        let own = f.name.as_ref().and_then(|n| self.own_symbol(n.span, &[SymbolKind::Function, SymbolKind::Method]));
        self.existing(self.infer_return(f).or_else(|| own.and_then(|s| self.declared_by_doc(s)))?)
    }

    /// Type for parameter `i` of `f`.
    pub fn param_type(&self, f: &FunctionDecl, i: usize) -> Option<SuggestedType> {
        let p = f.params.get(i)?;
        if p.ty.is_some() {
            return None;
        }
        let null_default = p.default.as_ref().is_some_and(|d| self.is_null(d));
        let own = f.name.as_ref().and_then(|n| self.own_symbol(n.span, &[SymbolKind::Function, SymbolKind::Method]));
        if let (Some(m), Some(class)) = (own.filter(|s| s.kind == SymbolKind::Method), self.class_at(f.span.start)) {
            if !m.name.eq_ignore_ascii_case("__construct") {
                match inherit::parent_param(self.index, &class.fqn, &m.name, i, p.variadic) {
                    inherit::Inherited::Typed(mut t) => {
                        if null_default {
                            t.add(Atom::Null);
                        }
                        return Some(t);
                    }
                    inherit::Inherited::Untyped => return None,
                    inherit::Inherited::Nothing => {}
                }
            }
        }
        let container = self.class_at(f.span.start).map(|c| c.fqn.as_str());
        let mut ty = own
            .and_then(|s| s.params.get(i))
            .filter(|info| !info.ty_native)
            .and_then(|info| text::type_text(info.ty.as_deref()?, &info.classes, container))
            .filter(|t| !t.need_classes && !t.ty.is_empty())
            .map(|t| t.ty);
        if p.variadic {
            // `@param int ...$ids` describes one element; `@param int[] $ids` the whole list
            ty = ty.and_then(variadic_element);
        }
        if let Some(d) = p.default.as_ref().filter(|_| !null_default) {
            match (self.constant_value(d), &ty) {
                (Some(lit), Some(t)) if !inherit::subtype(self.index, &lit, t, container.unwrap_or("")) => return None,
                (Some(lit), None) => ty = Some(widen_bool(lit)),
                (None, None) => return None,
                _ => {}
            }
        }
        let mut ty = self.existing(ty?)?;
        if null_default {
            ty.add(Atom::Null);
        }
        Some(ty)
    }

    /// Type for the property `p` (a declaration of one property only).
    pub fn property_type(&self, p: &PropertyDecl) -> Option<PropertyType> {
        if p.ty.is_some() {
            return None;
        }
        let class = self.class_at(p.span.start)?;
        let name = p.name.text.as_str();
        let null_default = p.default.as_ref().is_some_and(|d| self.is_null(d));
        match inherit::parent_property(self.index, &class.fqn, name) {
            inherit::Inherited::Typed(t) => {
                let add_null_default = p.default.is_none() && t.contains(&Atom::Null);
                return Some(PropertyType { ty: t, add_null_default });
            }
            inherit::Inherited::Untyped => return None,
            inherit::Inherited::Nothing => {}
        }
        let own = self.own_symbol(p.name.span, &[SymbolKind::Property]);
        let mut ty = own.and_then(|s| self.declared_by_doc(s));
        if let Some(d) = p.default.as_ref().filter(|_| !null_default) {
            match (self.constant_value(d), &ty) {
                (Some(lit), Some(t)) if !inherit::subtype(self.index, &lit, t, &class.fqn) => return None,
                (Some(lit), None) => ty = Some(lit),
                (None, None) => return None,
                _ => {}
            }
        }
        let mut ty = self.existing(widen_bool(ty?))?;
        let add_null_default = p.default.is_none();
        if null_default || add_null_default {
            ty.add(Atom::Null);
        }
        if !inherit::children_keep_property(self.index, &class.fqn, name, &ty) {
            return None;
        }
        Some(PropertyType { ty, add_null_default })
    }

    /// `throw new X` class names (as written) that are not inside a `try` block of `f`.
    pub fn thrown(&self, f: &FunctionDecl) -> Vec<String> {
        facts::collect(self, f).thrown
    }

    /// The docblock type of a symbol whose type is not declared natively.
    fn declared_by_doc(&self, s: &Symbol) -> Option<SuggestedType> {
        if s.ty_native {
            return None;
        }
        let t = text::type_text(s.ty.as_deref()?, &s.type_classes, s.container.as_deref())?;
        (!t.need_classes && !t.ty.is_empty()).then_some(t.ty)
    }

    /// `t` when every class in it exists; a docblock may name a type alias (`@phpstan-type`) or a
    /// class that is not installed. Element types of arrays that do not exist are just dropped.
    fn existing(&self, t: SuggestedType) -> Option<SuggestedType> {
        let mut out = SuggestedType::default();
        for a in t.atoms() {
            match a {
                // the two classes this module itself produces exist in every PHP
                Atom::Class(c) if c == "Closure" || c == "Generator" => out.add(a.clone()),
                Atom::Class(c) if self.index.find_class(c).is_none() => return None,
                Atom::ArrayOf(c) if self.index.find_class(c).is_none() => {}
                other => out.add(other.clone()),
            }
        }
        Some(out)
    }

    fn is_null(&self, e: &Expr) -> bool {
        matches!(&e.kind, php_syntax::ExprKind::Name(n) if n.text.trim_start_matches('\\').eq_ignore_ascii_case("null"))
    }
}

/// `true` / `false` -> `bool`: a default value or docblock rarely means the literal type.
fn widen_bool(mut t: SuggestedType) -> SuggestedType {
    if t.contains(&Atom::True) || t.contains(&Atom::False) {
        t.remove(&Atom::True);
        t.remove(&Atom::False);
        t.add(Atom::Bool);
    }
    t
}

/// `int[]` -> `int` for a variadic parameter's element; a plain `int` stays.
fn variadic_element(t: SuggestedType) -> Option<SuggestedType> {
    let mut out = SuggestedType::default();
    for a in t.atoms() {
        match a {
            Atom::ArrayOf(c) => out.add(Atom::Class(c.clone())),
            Atom::Array => return None,
            other => out.add(other.clone()),
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(c: &str) -> String {
        c.rsplit('\\').next().unwrap_or(c).to_string()
    }

    #[test]
    fn versions_from_constraints() {
        assert_eq!(PhpVersion::from_constraint("^7.4|^8.0"), Some(PhpVersion(7, 4)));
        assert_eq!(PhpVersion::from_constraint(">=8.1 <8.4"), Some(PhpVersion(8, 1)));
        assert_eq!(PhpVersion::from_constraint("~8.2.0"), Some(PhpVersion(8, 2)));
        assert_eq!(PhpVersion::from_constraint("8.*"), Some(PhpVersion(8, 0)));
        assert_eq!(PhpVersion::from_constraint("^8.0 || ^7.3"), Some(PhpVersion(7, 3)));
        assert_eq!(PhpVersion::from_constraint(">=7.2.5"), Some(PhpVersion(7, 2)));
        assert_eq!(PhpVersion::from_constraint("*"), None);
    }

    #[test]
    fn native_rendering_follows_versions() {
        let t = SuggestedType::of([Atom::Int, Atom::Null]);
        assert_eq!(t.native(PhpVersion(7, 0), Place::Return, &n), None);
        assert_eq!(t.native(PhpVersion(7, 1), Place::Return, &n).as_deref(), Some("?int"));
        let u = SuggestedType::of([Atom::Int, Atom::String]);
        assert_eq!(u.native(PhpVersion(7, 4), Place::Return, &n), None);
        assert_eq!(u.native(PhpVersion(8, 0), Place::Return, &n).as_deref(), Some("int|string"));
        let b = SuggestedType::of([Atom::True, Atom::False]);
        assert_eq!(b.native(PhpVersion(7, 0), Place::Param, &n).as_deref(), Some("bool"));
        let f = SuggestedType::of([Atom::String, Atom::False]);
        assert_eq!(f.native(PhpVersion(8, 0), Place::Return, &n).as_deref(), Some("string|false"));
        assert_eq!(SuggestedType::of([Atom::False]).native(PhpVersion(8, 1), Place::Return, &n), None);
        let c = SuggestedType::of([Atom::Class("App\\User".into()), Atom::Null]);
        assert_eq!(c.native(PhpVersion(7, 4), Place::Property, &n).as_deref(), Some("?User"));
        assert_eq!(c.native(PhpVersion(7, 3), Place::Property, &n), None);
        assert_eq!(
            SuggestedType::of([Atom::Void]).native(PhpVersion(7, 1), Place::Return, &n).as_deref(),
            Some("void")
        );
        assert_eq!(SuggestedType::of([Atom::Static]).native(PhpVersion(7, 4), Place::Return, &n), None);
        assert_eq!(SuggestedType::of([Atom::Callable]).native(PhpVersion(8, 0), Place::Property, &n), None);
    }

    #[test]
    fn doc_rendering_keeps_element_types() {
        let t = SuggestedType::of([Atom::Array, Atom::ArrayOf("App\\User".into()), Atom::Null]);
        assert_eq!(t.doc(&n), "User[]|null");
        assert_eq!(t.native(PhpVersion(8, 0), Place::Return, &n).as_deref(), Some("?array"));
    }
}
