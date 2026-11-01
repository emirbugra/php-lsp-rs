//! Symbol extraction: AST -> flat list of declarations.
//!
//! - `collect.rs`: walks the file and creates a [`Symbol`] for every class, function, method,
//!   property and constant
//! - `signature.rs`: parameter and return types (native hint + docblock) and the signature text
//! - `doc_members.rs`: `@property` / `@method` tags of a class docblock
//! - `eloquent.rs`: Laravel model members that only exist at runtime (relations, accessors, scopes, `$casts` …)

mod collect;
mod doc_members;
mod eloquent;
mod signature;

use crate::istr::IStr;
use php_syntax::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum SymbolKind {
    Class,
    Interface,
    Trait,
    Enum,
    Function,
    Method,
    Property,
    ClassConst,
    EnumCase,
    Const,
}

impl SymbolKind {
    pub fn is_class_like(self) -> bool {
        matches!(self, SymbolKind::Class | SymbolKind::Interface | SymbolKind::Trait | SymbolKind::Enum)
    }
}

#[derive(Clone, Debug, Default)]
pub struct ParamInfo {
    pub name: IStr,
    pub ty: Option<IStr>,
    pub classes: Vec<IStr>,
    pub default: Option<IStr>,
    pub variadic: bool,
    pub by_ref: bool,
    /// `ty` is written in the signature (not taken from the docblock).
    pub ty_native: bool,
}

#[derive(Clone, Debug)]
pub struct Symbol {
    pub kind: SymbolKind,
    pub name: IStr,
    /// Fully qualified name (classes, functions, constants). For members equal to `name`.
    pub fqn: IStr,
    /// FQN of the containing class-like for members.
    pub container: Option<IStr>,
    pub range: Span,
    pub name_span: Span,
    pub vis: Visibility,
    pub is_static: bool,
    pub is_abstract: bool,
    pub deprecated: bool,
    /// Declared in a docblock (@property / @method).
    pub is_virtual: bool,
    /// Class docblock has `@mixin` (members may be forwarded to another class).
    pub mixin: bool,
    /// Type display text (return type, property type, constant value type).
    pub ty: Option<IStr>,
    /// `ty` is written in the declaration (not taken from the docblock).
    pub ty_native: bool,
    /// Resolved class FQNs of `ty`. May contain `static`.
    pub type_classes: Vec<IStr>,
    pub params: Vec<ParamInfo>,
    /// One-line signature for hover / completion detail.
    pub detail: IStr,
    pub doc: Option<Span>,
    /// extends (+ interface extends), implements, used traits — resolved FQNs.
    /// Only class-likes carry these (boxed so members don't pay for three empty Vecs).
    pub bases: Option<Box<Bases>>,
}

#[derive(Clone, Debug, Default)]
pub struct Bases {
    pub extends: Vec<IStr>,
    pub implements: Vec<IStr>,
    pub traits: Vec<IStr>,
    /// `@template T of Bound`: (name, encoded bound atoms joined by `|`). Classes and generic functions.
    pub templates: Vec<(IStr, IStr)>,
    /// `@extends/@implements/@use X<...>` as encoded generic atoms.
    pub inherit: Vec<IStr>,
}

impl Symbol {
    pub fn extends(&self) -> &[IStr] {
        self.bases.as_ref().map(|b| &b.extends[..]).unwrap_or(&[])
    }

    pub fn implements(&self) -> &[IStr] {
        self.bases.as_ref().map(|b| &b.implements[..]).unwrap_or(&[])
    }

    pub fn templates(&self) -> &[(IStr, IStr)] {
        self.bases.as_ref().map(|b| &b.templates[..]).unwrap_or(&[])
    }

    pub fn inherit(&self) -> &[IStr] {
        self.bases.as_ref().map(|b| &b.inherit[..]).unwrap_or(&[])
    }

    pub fn traits(&self) -> &[IStr] {
        self.bases.as_ref().map(|b| &b.traits[..]).unwrap_or(&[])
    }

    fn new(kind: SymbolKind, name: String, fqn: String, range: Span, name_span: Span) -> Self {
        Symbol {
            kind,
            name: IStr::from(name),
            fqn: IStr::from(fqn),
            container: None,
            range,
            name_span,
            vis: Visibility::Public,
            is_static: false,
            is_abstract: false,
            deprecated: false,
            is_virtual: false,
            mixin: false,
            ty: None,
            ty_native: false,
            type_classes: Vec::new(),
            params: Vec::new(),
            detail: IStr::from(String::new()),
            doc: None,
            bases: None,
        }
    }

    /// A member of class `class_fqn` (its `fqn` is the plain name).
    fn member(kind: SymbolKind, name: String, class_fqn: &str, range: Span, name_span: Span) -> Self {
        let mut sym = Symbol::new(kind, name.clone(), name, range, name_span);
        sym.container = Some(IStr::from(class_fqn.to_string()));
        sym
    }
}

pub fn extract(src: &str, stmts: &[Stmt]) -> Vec<Symbol> {
    let mut c = collect::SymbolCollector::new(src);
    c.stmts(stmts);
    c.out
}
