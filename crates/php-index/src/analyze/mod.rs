//! Reference collection and lightweight type inference for one file.
//!
//! [`analyze`] walks the syntax tree once. For every name it meets (class, function, member,
//! variable) it records a [`Reference`] with the declarations it may point to, and it keeps track
//! of the types of variables along the way.
//!
//! The work is split by topic:
//! - `scope.rs`: declaring and looking up variables, resolving class names
//! - `narrowing.rs`: types learned from conditions (`if ($x instanceof User)`)
//! - `infer.rs`: the type of an expression
//! - `conditional.rs`: conditional return types (`($x is null ? A : B)`) picked by the call
//! - `hints.rs`: parameter types of closures passed to well-known functions / methods
//! - `doc_comments.rs`: class names and `@var` tags inside docblocks
//! - `visit.rs`: the tree walk itself

mod conditional;
mod doc_comments;
mod hints;
mod infer;
mod narrowing;
mod scope;
mod visit;

use crate::index::{Index, MemberKind, SymLoc};
use crate::names::NameCtx;
use crate::types::{ParamTypes, Types};
use php_syntax::walk::walk_stmts;
use php_syntax::*;
use rustc_hash::FxHashMap;
use std::cell::RefCell;

#[derive(Debug, Clone)]
pub enum RefTarget {
    Class(String),
    /// Candidate FQNs in lookup order.
    Function(Vec<String>),
    Const(Vec<String>),
    Member {
        kind: MemberKind,
        name: String,
        classes: Types,
    },
    Variable {
        name: String,
        decl: Option<Span>,
        classes: Types,
    },
}

#[derive(Debug, Clone)]
pub struct Reference {
    pub span: Span,
    pub target: RefTarget,
}

#[derive(Debug, Clone)]
pub struct ScopeSnapshot {
    pub span: Span,
    pub vars: Vec<(String, Types)>,
}

/// A class-like declaration: where it is, its FQN and the class it extends.
#[derive(Debug, Clone)]
pub struct ClassScope {
    pub span: Span,
    pub fqn: String,
    pub parent: Option<String>,
}

#[derive(Debug, Default)]
pub struct Analysis {
    pub refs: Vec<Reference>,
    pub scopes: Vec<ScopeSnapshot>,
    /// Every class-like declared in the file.
    pub classes: Vec<ClassScope>,
    /// Name context at the end of the file.
    pub ctx: NameCtx,
    /// Class types of every `return` value and arrow-function body, keyed by the expression's span.
    /// Used to suggest return types; scalars are not represented (see [`Types`]).
    pub returns: Vec<(Span, Types)>,
    /// Class types of the expressions asked for with [`analyze_with`], keyed by their span.
    pub expr_types: Vec<(Span, Types)>,
}

impl Analysis {
    /// Innermost reference containing `offset`.
    pub fn ref_at(&self, offset: usize) -> Option<&Reference> {
        self.refs
            .iter()
            .filter(|r| r.span.contains(offset))
            .min_by_key(|r| (r.span.len(), std::cmp::Reverse(r.span.start)))
    }

    /// Innermost class-like containing `offset`.
    pub fn class_at(&self, offset: usize) -> Option<&ClassScope> {
        self.classes.iter().filter(|c| c.span.contains(offset)).min_by_key(|c| c.span.len())
    }

    /// Class types recorded for the returned expression at `span`.
    pub fn returned_classes(&self, span: Span) -> Option<&Types> {
        self.returns.iter().find(|(s, _)| *s == span).map(|(_, t)| t)
    }

    /// Variables visible in the innermost scope containing `offset`.
    pub fn vars_at(&self, offset: usize) -> Option<&ScopeSnapshot> {
        self.scopes.iter().filter(|s| s.span.contains(offset)).min_by_key(|s| s.span.len())
    }
}

pub fn analyze(index: &Index, src: &str, stmts: &[Stmt]) -> Analysis {
    analyze_with(index, src, stmts, &[], &[])
}

/// [`analyze`] with variables that exist before the first statement (the data a controller passes
/// to a Blade view), recording the types of the expressions at `want` in [`Analysis::expr_types`].
pub fn analyze_with(index: &Index, src: &str, stmts: &[Stmt], seed: &[(String, Types)], want: &[Span]) -> Analysis {
    let mut top = Scope { vars: FxHashMap::default(), span: Span::new(0, src.len()) };
    for (name, classes) in seed {
        top.vars.insert(name.clone(), VarInfo { classes: classes.clone(), decl: Span::new(0, 0) });
    }
    let mut a = Analyzer {
        index,
        src,
        ctx: NameCtx::default(),
        classes: Vec::new(),
        scopes: vec![top],
        out: Analysis::default(),
        pending_doc_type: None,
        doc_vars: Vec::new(),
        narrowed: RefCell::new(Vec::new()),
        closure_hint: None,
        branch_depth: 0,
        seen_docs: Default::default(),
        want: want.iter().map(|s| (s.start, s.end)).collect(),
    };
    walk_stmts(&mut a, stmts);
    let top = a.scopes.pop().unwrap();
    a.snapshot(top);
    a.out.ctx = a.ctx;
    a.out
}

#[derive(Clone)]
struct VarInfo {
    classes: Types,
    decl: Span,
}

/// Variables of one function body (or of the file's top level).
struct Scope {
    vars: FxHashMap<String, VarInfo>,
    span: Span,
}

struct Analyzer<'a> {
    index: &'a Index,
    src: &'a str,
    ctx: NameCtx,
    /// Classes we are inside of, innermost last.
    classes: Vec<ClassScope>,
    scopes: Vec<Scope>,
    out: Analysis,
    /// `@var Type` (without a variable name) of the current statement.
    pending_doc_type: Option<Types>,
    /// `@var Type $name` of the current statement; wins over inferred types.
    doc_vars: Vec<(String, Types)>,
    /// Types learned from conditions, keyed by expression path (`$x`, `$this->a->b`); later entries
    /// win and `None` forgets one. It is a stack: leaving a branch truncates it to an earlier mark.
    narrowed: RefCell<Vec<(String, Option<Types>)>>,
    /// Parameter types for the next closure literal visited (from the call it is passed to).
    closure_hint: Option<ParamTypes>,
    /// > 0 while inside a branch / loop body: assignments there add to a variable's types instead of replacing them.
    branch_depth: u32,
    /// Docblocks whose class names were already turned into references (by start offset).
    seen_docs: rustc_hash::FxHashSet<u32>,
    /// Expression spans whose types go to [`Analysis::expr_types`].
    want: rustc_hash::FxHashSet<(u32, u32)>,
}

fn push_unique(v: &mut Types, s: String) {
    if !v.iter().any(|x| x.eq_ignore_ascii_case(&s)) {
        v.push(s);
    }
}

impl Index {
    /// Resolve a reference target to declarations.
    pub fn resolve(&self, t: &RefTarget) -> Vec<SymLoc> {
        match t {
            RefTarget::Class(fqn) => self.find_classes(fqn).to_vec(),
            RefTarget::Function(cands) => cands.iter().find_map(|c| self.find_function(c)).into_iter().collect(),
            RefTarget::Const(cands) => cands.iter().find_map(|c| self.find_const(c)).into_iter().collect(),
            RefTarget::Member { kind, name, classes } => {
                let mut out = Vec::new();
                for c in classes {
                    if let Some(l) = self.find_member(c, name, *kind) {
                        if !out.contains(&l) {
                            out.push(l);
                        }
                    }
                }
                out
            }
            RefTarget::Variable { .. } => Vec::new(),
        }
    }
}
