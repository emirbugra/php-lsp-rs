//! Types forced by parent classes and forbidden by subclasses.
//!
//! PHP checks signatures against the parent: parameter types may only get wider, return types
//! only narrower, property types must stay the same. So
//! - a parameter whose parent counterpart is untyped cannot get a type, and a typed one gets
//!   exactly the parent's type;
//! - a return type must fit the parent's, and every subclass override must already declare a
//!   return type that fits the new one;
//! - a property redeclared in a parent or subclass must use the same type there.

use super::{text, Atom, SuggestedType};
use crate::index::{Index, MemberKind};
use crate::symbols::Symbol;
use php_syntax::Visibility;

pub(super) enum Inherited {
    /// No parent declares the member.
    Nothing,
    /// A parent declares it without a type.
    Untyped,
    /// A parent declares it with this type.
    Typed(SuggestedType),
}

/// The member declared by `class` itself (not inherited).
fn own_member<'i>(index: &'i Index, class: &str, name: &str, kind: MemberKind) -> Option<&'i Symbol> {
    let m = index.sym(index.find_member_direct(class, name, kind)?);
    m.container.as_deref().is_some_and(|c| c.eq_ignore_ascii_case(class)).then_some(m)
}

/// Same-named members of parent classes, interfaces and used traits (not the class itself). Traits
/// count: PHP checks a property against the one a trait declares (Laravel's `$fillable` lives in
/// the `GuardsAttributes` trait), and a method against the trait method its parent got.
fn overridden<'i>(index: &'i Index, class: &str, name: &str, kind: MemberKind) -> Vec<&'i Symbol> {
    index
        .ancestors(class)
        .into_iter()
        .map(|l| index.sym(l))
        .filter(|c| !c.fqn.eq_ignore_ascii_case(class))
        .filter_map(|c| own_member(index, &c.fqn, name, kind))
        .filter(|m| m.vis != Visibility::Private)
        .collect()
}

/// Same-named members redeclared in subclasses (and implementations of an interface).
fn overriding<'i>(index: &'i Index, class: &str, name: &str, kind: MemberKind) -> Vec<&'i Symbol> {
    index.descendants(class).into_iter().filter_map(|l| own_member(index, &index.sym(l).fqn, name, kind)).collect()
}

/// The signature type of a member (`self` means its own class); None when untyped.
fn native_type(s: &Symbol) -> Option<SuggestedType> {
    if !s.ty_native {
        return None;
    }
    let p = text::type_text(s.ty.as_deref()?, &s.type_classes, s.container.as_deref())?;
    let mut ty = p.ty;
    if p.need_classes {
        ty.add(Atom::Static);
    }
    Some(ty)
}

pub(super) fn parent_return(index: &Index, class: &str, method: &str) -> Inherited {
    let parents = overridden(index, class, method, MemberKind::Method);
    match parents.iter().find_map(|m| native_type(m)) {
        Some(t) => Inherited::Typed(t),
        None if parents.is_empty() => Inherited::Nothing,
        None => Inherited::Untyped,
    }
}

pub(super) fn parent_param(index: &Index, class: &str, method: &str, i: usize, variadic: bool) -> Inherited {
    let mut found = None;
    for m in overridden(index, class, method, MemberKind::Method) {
        let p = m.params.get(i).or_else(|| m.params.last().filter(|p| p.variadic && variadic));
        let Some(p) = p else { continue };
        if !p.ty_native {
            return Inherited::Untyped;
        }
        let Some(t) = p.ty.as_deref().and_then(|t| text::type_text(t, &p.classes, m.container.as_deref())) else {
            return Inherited::Untyped;
        };
        if t.need_classes {
            return Inherited::Untyped;
        }
        found.get_or_insert(t.ty);
    }
    found.map(Inherited::Typed).unwrap_or(Inherited::Nothing)
}

pub(super) fn parent_property(index: &Index, class: &str, name: &str) -> Inherited {
    let parents = overridden(index, class, name, MemberKind::Property);
    match parents.first() {
        None => Inherited::Nothing,
        Some(p) => native_type(p).map(Inherited::Typed).unwrap_or(Inherited::Untyped),
    }
}

/// Every override of `method` in a subclass declares a return type that fits `ty`.
pub(super) fn children_accept_return(index: &Index, class: &str, method: &str, ty: &SuggestedType) -> bool {
    overriding(index, class, method, MemberKind::Method).into_iter().all(|m| {
        let child_class = m.container.as_deref().unwrap_or("");
        native_type(m).is_some_and(|t| subtype(index, &t, ty, child_class))
    })
}

/// Every subclass that redeclares the property declares it with exactly `ty`.
pub(super) fn children_keep_property(index: &Index, class: &str, name: &str, ty: &SuggestedType) -> bool {
    overriding(index, class, name, MemberKind::Property).into_iter().all(|p| native_type(p).as_ref() == Some(ty))
}

/// Is every value of `sub` also a value of `sup`? `class` is what `static` / `self` mean in `sub`.
pub(super) fn subtype(index: &Index, sub: &SuggestedType, sup: &SuggestedType, class: &str) -> bool {
    let sup_atoms = sup.simplified();
    let has = |a: &Atom| sup_atoms.contains(a);
    let class_fits = |c: &str| {
        has(&Atom::Object)
            || sup_atoms.iter().any(|s| match s {
                Atom::Class(p) => p.eq_ignore_ascii_case(c) || index.is_subclass_of(c, p),
                Atom::SelfType | Atom::Static => c.eq_ignore_ascii_case(class) || index.is_subclass_of(c, class),
                Atom::Callable => c.eq_ignore_ascii_case("Closure"),
                Atom::Iterable => index.is_subclass_of(c, "Traversable"),
                _ => false,
            })
    };
    if has(&Atom::Mixed) {
        return !sub.contains(&Atom::Void);
    }
    sub.simplified().iter().all(|a| match a {
        Atom::Never => true,
        Atom::True | Atom::False => has(a) || has(&Atom::Bool),
        Atom::Array | Atom::ArrayOf(_) => has(&Atom::Array) || has(&Atom::Iterable),
        Atom::Class(c) => class_fits(c),
        Atom::SelfType | Atom::Static => class_fits(class),
        Atom::Mixed => false,
        other => has(other),
    })
}
