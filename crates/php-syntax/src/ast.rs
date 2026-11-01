use crate::span::Span;

#[derive(Debug, Clone)]
pub struct Ident {
    pub text: String,
    pub span: Span,
}

/// A (possibly qualified) name exactly as written in source.
#[derive(Debug, Clone)]
pub struct Name {
    pub text: String,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
    /// Doc comment immediately preceding the statement.
    pub doc: Option<Span>,
}

impl Stmt {
    /// `return` / `break` / `continue` for those statements (they are parsed as [`StmtKind::Exprs`]).
    pub fn jump_keyword(&self, src: &str) -> Option<&'static str> {
        if !matches!(self.kind, StmtKind::Exprs(_)) {
            return None;
        }
        let t = self.span.text(src).trim_start().as_bytes();
        ["return", "break", "continue"].into_iter().find(|k| {
            t.len() >= k.len()
                && t[..k.len()].eq_ignore_ascii_case(k.as_bytes())
                && !t.get(k.len()).is_some_and(|&c| c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80)
        })
    }

    /// The value of a `return` statement: `Some(None)` for a bare `return;`, `None` for other statements.
    pub fn return_value(&self, src: &str) -> Option<Option<&Expr>> {
        match &self.kind {
            StmtKind::Exprs(es) if self.jump_keyword(src) == Some("return") => Some(es.first()),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum StmtKind {
    InlineHtml,
    /// echo / return / global / unset / static vars / break / continue ...
    Exprs(Vec<Expr>),
    Expr(Expr),
    Namespace {
        name: Option<Name>,
        body: Option<Vec<Stmt>>,
    },
    Use(Vec<UseItem>),
    Function(Box<FunctionDecl>),
    Class(Box<ClassDecl>),
    Const(Vec<ConstItem>),
    /// if / while / for / foreach / switch / declare. `header` holds every header expression,
    /// `blocks` every branch body in source order.
    Control {
        kw: &'static str,
        header: Vec<Expr>,
        blocks: Vec<Vec<Stmt>>,
    },
    Try {
        body: Vec<Stmt>,
        catches: Vec<Catch>,
        finally: Option<Vec<Stmt>>,
    },
    Block(Vec<Stmt>),
    Nop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UseKind {
    Class,
    Function,
    Const,
}

#[derive(Debug, Clone)]
pub struct UseItem {
    pub kind: UseKind,
    /// Fully qualified, without leading backslash.
    pub name: String,
    pub alias: Option<String>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ConstItem {
    pub name: Ident,
    pub value: Expr,
    pub doc: Option<Span>,
}

#[derive(Debug, Clone)]
pub struct Catch {
    pub types: Vec<Name>,
    pub var: Option<Ident>,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, Default)]
pub struct TypeHint {
    pub names: Vec<Name>,
    pub nullable: bool,
    pub text: String,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub span: Span,
    pub ty: Option<TypeHint>,
    pub default: Option<Expr>,
    pub by_ref: bool,
    pub variadic: bool,
    pub promoted: Option<Visibility>,
    pub attrs: Vec<Expr>,
}

#[derive(Debug, Clone)]
pub struct ClosureUse {
    pub name: String,
    pub span: Span,
    pub by_ref: bool,
}

#[derive(Debug, Clone, Default)]
pub struct FunctionDecl {
    pub name: Option<Ident>,
    pub params: Vec<Param>,
    pub ret: Option<TypeHint>,
    pub by_ref: bool,
    pub uses: Vec<ClosureUse>,
    pub body: Option<Vec<Stmt>>,
    pub arrow_body: Option<Box<Expr>>,
    pub is_static: bool,
    pub attrs: Vec<Expr>,
    pub doc: Option<Span>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Visibility {
    Public,
    Protected,
    Private,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassKind {
    Class,
    Interface,
    Trait,
    Enum,
}

#[derive(Debug, Clone, Default)]
pub struct Modifiers {
    pub vis: Option<Visibility>,
    pub is_static: bool,
    pub is_abstract: bool,
    pub is_final: bool,
    pub is_readonly: bool,
}

#[derive(Debug, Clone)]
pub struct ClassDecl {
    pub kind: ClassKind,
    pub name: Option<Ident>,
    pub modifiers: Modifiers,
    pub extends: Vec<Name>,
    pub implements: Vec<Name>,
    pub backing: Option<TypeHint>,
    pub members: Vec<Member>,
    pub attrs: Vec<Expr>,
    pub doc: Option<Span>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum Member {
    Method { decl: Box<FunctionDecl>, modifiers: Modifiers },
    Property(PropertyDecl),
    Const { items: Vec<ConstItem>, modifiers: Modifiers, span: Span },
    TraitUse { names: Vec<Name>, span: Span },
    Case { name: Ident, value: Option<Expr>, doc: Option<Span>, span: Span },
}

#[derive(Debug, Clone)]
pub struct PropertyDecl {
    pub name: Ident, // text without '$', span covers '$name'
    pub ty: Option<TypeHint>,
    pub default: Option<Expr>,
    pub modifiers: Modifiers,
    pub doc: Option<Span>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

/// Content of a quoted string literal: `'abc'` / `"abc"` -> `abc` (escapes are left as written).
pub fn string_content(raw: &str) -> Option<&str> {
    let quote = raw.chars().next()?;
    ((quote == '\'' || quote == '"') && raw.len() >= 2 && raw.ends_with(quote)).then(|| &raw[1..raw.len() - 1])
}

impl Expr {
    /// Content of this expression when it is a plain string literal.
    pub fn string_value<'s>(&self, src: &'s str) -> Option<&'s str> {
        match self.kind {
            ExprKind::Lit => string_content(self.span.text(src)),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Arg {
    pub name: Option<Ident>,
    pub value: Expr,
    pub spread: bool,
}

#[derive(Debug, Clone)]
pub enum MemberName {
    Ident(Ident),
    Expr(Box<Expr>),
}

#[derive(Debug, Clone)]
pub enum ClassRef {
    Name(Name),
    Expr(Box<Expr>),
    Anon(Box<ClassDecl>),
}

#[derive(Debug, Clone)]
pub struct ArrayItem {
    pub key: Option<Expr>,
    pub value: Expr,
    pub by_ref: bool,
    pub spread: bool,
}

#[derive(Debug, Clone)]
pub struct MatchArm {
    /// Empty for `default`.
    pub conds: Vec<Expr>,
    pub body: Expr,
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    Var(String),
    VarVar(Box<Expr>),
    Name(Name),
    Lit,
    Interp(Vec<Expr>),
    Array(Vec<ArrayItem>),
    Call { callee: Box<Expr>, args: Vec<Arg> },
    New { class: ClassRef, args: Vec<Arg> },
    MethodCall { obj: Box<Expr>, name: MemberName, args: Vec<Arg>, nullsafe: bool },
    Prop { obj: Box<Expr>, name: MemberName, nullsafe: bool },
    StaticCall { class: ClassRef, name: MemberName, args: Vec<Arg> },
    StaticProp { class: ClassRef, name: MemberName },
    ClassConst { class: ClassRef, name: Ident },
    Index { base: Box<Expr>, index: Option<Box<Expr>> },
    Binary { op: &'static str, lhs: Box<Expr>, rhs: Box<Expr> },
    Unary { op: &'static str, expr: Box<Expr> },
    Cast { ty: &'static str, expr: Box<Expr> },
    Assign { target: Box<Expr>, value: Box<Expr>, op: &'static str, by_ref: bool },
    Ternary { cond: Box<Expr>, then: Option<Box<Expr>>, els: Box<Expr> },
    Closure(Box<FunctionDecl>),
    Match { subject: Box<Expr>, arms: Vec<MatchArm> },
    InstanceOf { expr: Box<Expr>, class: ClassRef },
    Isset(Vec<Expr>),
    Empty(Box<Expr>),
    Exit(Option<Box<Expr>>),
    Include(Box<Expr>),
    Clone(Box<Expr>),
    Print(Box<Expr>),
    Throw(Box<Expr>),
    Yield { key: Option<Box<Expr>>, value: Option<Box<Expr>> },
    YieldFrom(Box<Expr>),
    Paren(Box<Expr>),
    Error,
}
