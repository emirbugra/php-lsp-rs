//! Spaces between tokens on one line, and the letter case of keywords and `true` / `false` /
//! `null`.
//!
//! Every gap between two tokens on the same line gets a decision: no space, one space, or "keep"
//! (any run of spaces / tabs becomes one space, no space stays none). The decision depends on the
//! two tokens and a little context: what a `(` belongs to (call, declaration, `if` …), whether an
//! operator is unary or binary, whether a `?` / `:` is a ternary, a nullable type, a return type
//! or a named argument, and which tokens form a type (`int|string`, `?Foo`, `A&B`). Gaps next to
//! HTML and PHP tags, and gaps holding a line break, are left alone (line breaks belong to
//! `indent.rs`); in front of a trailing comment only tabs are replaced.

use crate::options::FormatOptions;
use php_syntax::lexer::{lex_with_trivia, Tok, Token};

/// Replace `src[start..end]` with `text`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Edit {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Want {
    None,
    One,
    /// Spaces and tabs become one space; no space stays none.
    Keep,
    /// Leave the gap exactly as written.
    AsIs,
}

fn space(on: bool) -> Want {
    if on {
        Want::One
    } else {
        Want::None
    }
}

/// What an opening bracket belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Ctx {
    Call,
    /// Parameter list of a named function / method.
    Decl,
    /// Parameter list of `function (…)` / `fn (…)`.
    ClosureDecl,
    /// `use (…)` of a closure.
    ClosureUse,
    If,
    While,
    For,
    Switch,
    Catch,
    Declare,
    /// `array(…)` / `list(…)`.
    ArrayCall,
    Group,
    Index,
    ArrayLit,
    Attr,
    Block,
    /// `->{…}`, `${…}`, group `use A\{…}`.
    BraceExpr,
    Root,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum Role {
    #[default]
    Plain,
    /// Part of a type declaration (`?`, `|`, `&`, `(`, `)`, names).
    Type,
    /// Unary / prefix operator (also `&` by reference and `...`).
    Unary,
    /// Postfix `++` / `--`.
    Postfix,
    Binary,
    TernaryQuest,
    TernaryColon,
    ReturnColon,
    NamedArgColon,
    EnumColon,
    /// `case …:`, `default:`, `else:`, `if (…):`, `label:`.
    LabelColon,
}

struct Frame {
    ctx: Ctx,
    ternary: u32,
}

/// Keywords after which an expression starts (so a following `-` / `&` is unary, `(` groups).
const EXPR_KEYWORDS: &[&str] = &[
    "return",
    "echo",
    "print",
    "case",
    "yield",
    "throw",
    "include",
    "include_once",
    "require",
    "require_once",
    "new",
    "else",
    "and",
    "or",
    "xor",
    "instanceof",
    "clone",
    "as",
    "function",
    "fn",
    "use",
    "global",
    "extends",
    "implements",
    "insteadof",
    "do",
    "from",
    "static",
    "const",
    "public",
    "protected",
    "private",
    "readonly",
    "var",
    "abstract",
    "final",
];

/// Constructs that take `(` like a call.
const CALL_LIKE: &[&str] = &["isset", "empty", "unset", "exit", "die", "eval", "array", "list"];

/// Modifiers in front of a property / promoted parameter type.
const MODIFIERS: &[&str] = &["public", "protected", "private", "readonly", "var", "static", "final", "abstract"];

/// Reserved words that `LOWER_CASE_KEYWORDS` lower-cases.
const KEYWORDS: &[&str] = &[
    "abstract",
    "and",
    "array",
    "as",
    "break",
    "callable",
    "case",
    "catch",
    "class",
    "clone",
    "const",
    "continue",
    "declare",
    "default",
    "die",
    "do",
    "echo",
    "else",
    "elseif",
    "empty",
    "enddeclare",
    "endfor",
    "endforeach",
    "endif",
    "endswitch",
    "endwhile",
    "eval",
    "exit",
    "extends",
    "final",
    "finally",
    "fn",
    "for",
    "foreach",
    "function",
    "global",
    "goto",
    "if",
    "implements",
    "include",
    "include_once",
    "instanceof",
    "insteadof",
    "interface",
    "isset",
    "list",
    "namespace",
    "new",
    "or",
    "print",
    "private",
    "protected",
    "public",
    "readonly",
    "require",
    "require_once",
    "return",
    "static",
    "switch",
    "throw",
    "trait",
    "try",
    "unset",
    "use",
    "var",
    "while",
    "xor",
    "yield",
];

/// Is `word` (lower case) a keyword whose letter case the formatter may change?
pub(crate) fn case_insensitive_word(word: &str) -> bool {
    KEYWORDS.contains(&word) || matches!(word, "true" | "false" | "null")
}

struct Spacer<'a> {
    src: &'a str,
    toks: &'a [Token],
    o: &'a FormatOptions,
    /// Significant tokens (no comments / tags) as indices into `toks`.
    sig: Vec<usize>,
    role: Vec<Role>,
    /// For brackets: what they belong to (by significant position).
    ctx: Vec<Option<Ctx>>,
    /// What the innermost bracket around each token belongs to.
    encl: Vec<Ctx>,
    /// Lower-cased identifiers that are not lower case already.
    low: Vec<Option<String>>,
}

impl<'a> Spacer<'a> {
    fn text(&self, k: usize) -> &'a str {
        self.toks[self.sig[k]].span.text(self.src)
    }

    fn kind(&self, k: usize) -> Tok {
        self.toks[self.sig[k]].kind
    }

    /// Text of an identifier in lower case (other tokens as they are).
    fn lower(&self, k: usize) -> &str {
        self.low[k].as_deref().unwrap_or_else(|| self.text(k))
    }

    fn is_word(&self, k: usize) -> bool {
        self.kind(k) == Tok::Ident
    }

    /// An identifier used as a name, not as a keyword (`$x->if`, `A::class`, `function list`).
    fn word_is_name(&self, k: usize) -> bool {
        k > 0 && (matches!(self.text(k - 1), "->" | "?->" | "::") || self.lower(k - 1) == "function")
    }

    /// Does significant token `k` end an operand (so an operator after it is binary)?
    fn ends_operand(&self, k: usize) -> bool {
        match self.kind(k) {
            Tok::Variable | Tok::Int | Tok::Float | Tok::Str | Tok::Template | Tok::Name => true,
            Tok::Ident => self.word_is_name(k) || !EXPR_KEYWORDS.contains(&self.lower(k)),
            Tok::Punct(")" | "]") => true,
            Tok::Punct("}") => self.ctx[k] == Some(Ctx::BraceExpr),
            Tok::Punct("++" | "--") => self.role[k] == Role::Postfix,
            _ => false,
        }
    }

    /// Mark the type starting at significant position `k` (`?Foo`, `int|string`, `(A&B)|null`).
    /// Returns the position after it.
    fn mark_type(&mut self, mut k: usize) -> usize {
        let n = self.sig.len();
        let mut depth = 0i32;
        while k < n {
            let ok = match self.kind(k) {
                Tok::Ident | Tok::Name => true,
                Tok::Punct("?" | "|") => true,
                // `A&B` is an intersection; `A &$x` / `A &...$x` passes by reference
                Tok::Punct("&") => k + 1 < n && matches!(self.kind(k + 1), Tok::Ident | Tok::Name | Tok::Punct("(")),
                Tok::Punct("(") => {
                    depth += 1;
                    true
                }
                Tok::Punct(")") if depth > 0 => {
                    depth -= 1;
                    true
                }
                _ => false,
            };
            if !ok {
                break;
            }
            self.role[k] = Role::Type;
            k += 1;
        }
        k
    }

    /// Skip attributes `#[…]` and modifiers (`public`, `private(set)`) at position `k`.
    fn skip_attrs_and_modifiers(&self, mut k: usize) -> usize {
        let n = self.sig.len();
        loop {
            if k < n && self.text(k) == "#[" {
                let mut depth = 0;
                while k < n {
                    match self.text(k) {
                        "#[" | "[" => depth += 1,
                        "]" => {
                            depth -= 1;
                            if depth == 0 {
                                k += 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                    k += 1;
                }
                continue;
            }
            if k < n && self.is_word(k) && MODIFIERS.contains(&self.lower(k)) {
                k += 1;
                // asymmetric visibility: `private(set)`
                if k + 2 < n && self.text(k) == "(" && self.lower(k + 1) == "set" && self.text(k + 2) == ")" {
                    k += 3;
                }
                continue;
            }
            return k;
        }
    }

    /// The bracket at `k` opens: what does it belong to?
    fn open_ctx(&self, k: usize) -> Ctx {
        let prev = k.checked_sub(1);
        let prev_lower = prev.map(|p| self.lower(p)).unwrap_or("");
        let prev_kw = prev.is_some_and(|p| self.is_word(p) && !self.word_is_name(p));
        let before_fn = |p: usize| p > 0 && matches!(self.lower(p - 1), "function" | "fn");
        match self.text(k) {
            "#[" => Ctx::Attr,
            "[" => match prev {
                Some(p) if self.ends_operand(p) => Ctx::Index,
                _ => Ctx::ArrayLit,
            },
            "{" => match prev {
                // `->{…}`, `${…}`, `A\{…}`, and the old string offset `$s{0}`, `$o->p{$i}`
                Some(p)
                    if matches!(self.text(p), "->" | "?->" | "::" | "$" | "]")
                        || self.text(p).ends_with('\\')
                        || self.kind(p) == Tok::Variable
                        || self.is_word(p) && self.word_is_name(p) =>
                {
                    Ctx::BraceExpr
                }
                _ => Ctx::Block,
            },
            _ => {
                let Some(p) = prev else { return Ctx::Group };
                if prev_kw {
                    match prev_lower {
                        "if" | "elseif" | "match" => return Ctx::If,
                        "while" => return Ctx::While,
                        "for" | "foreach" => return Ctx::For,
                        "switch" => return Ctx::Switch,
                        "catch" => return Ctx::Catch,
                        "declare" => return Ctx::Declare,
                        "use" => return Ctx::ClosureUse,
                        "function" | "fn" => return Ctx::ClosureDecl,
                        "array" | "list" => return Ctx::ArrayCall,
                        w if CALL_LIKE.contains(&w) => return Ctx::Call,
                        _ => {}
                    }
                }
                // `function name(`, `function &name(`, `fn&(`
                if self.text(p) == "&" && before_fn(p) {
                    return Ctx::ClosureDecl;
                }
                if self.is_word(p) && p > 0 && self.lower(p - 1) == "function" {
                    return Ctx::Decl;
                }
                if self.is_word(p) && p > 1 && self.text(p - 1) == "&" && self.lower(p - 2) == "function" {
                    return Ctx::Decl;
                }
                if prev_kw && EXPR_KEYWORDS.contains(&prev_lower) && prev_lower != "static" {
                    return Ctx::Group;
                }
                if self.ends_operand(p) || prev_lower == "static" {
                    Ctx::Call
                } else {
                    Ctx::Group
                }
            }
        }
    }

    /// Work out roles and bracket contexts for all significant tokens.
    fn analyze(&mut self) {
        let n = self.sig.len();
        let mut stack = vec![Frame { ctx: Ctx::Root, ternary: 0 }];
        // a class-like body: properties there may have types
        let mut class_bodies: Vec<usize> = Vec::new();
        let mut pending_class = false;
        let mut k = 0;
        while k < n {
            let text = self.text(k);
            let kind = self.kind(k);
            let prev = k.checked_sub(1);
            self.encl[k] = stack.last().expect("root").ctx;
            match kind {
                Tok::Punct("(" | "[" | "#[" | "{") => {
                    let ctx = self.open_ctx(k);
                    self.ctx[k] = Some(ctx);
                    if ctx == Ctx::Block && pending_class {
                        class_bodies.push(stack.len());
                        pending_class = false;
                    }
                    stack.push(Frame { ctx, ternary: 0 });
                    if matches!(ctx, Ctx::Decl | Ctx::ClosureDecl) {
                        self.param_type(k + 1);
                    } else if ctx == Ctx::Catch {
                        self.mark_type(k + 1);
                    } else if ctx == Ctx::Block && class_bodies.last() == Some(&(stack.len() - 1)) {
                        self.member_type(k + 1);
                    }
                }
                Tok::Punct(")" | "]" | "}") => {
                    if stack.len() > 1 {
                        let f = stack.pop().expect("checked");
                        self.ctx[k] = Some(f.ctx);
                        if class_bodies.last() == Some(&stack.len()) {
                            class_bodies.pop();
                        }
                        // return type after a parameter list / closure `use (…)`
                        if matches!(f.ctx, Ctx::Decl | Ctx::ClosureDecl | Ctx::ClosureUse)
                            && k + 1 < n
                            && self.text(k + 1) == ":"
                        {
                            self.role[k + 1] = Role::ReturnColon;
                            self.mark_type(k + 2);
                        }
                    }
                    if text == "}" && class_bodies.last() == Some(&(stack.len() - 1)) {
                        self.member_type(k + 1);
                    }
                }
                Tok::Punct(",") => {
                    let top = stack.last().expect("root").ctx;
                    if matches!(top, Ctx::Decl | Ctx::ClosureDecl) {
                        self.param_type(k + 1);
                    }
                }
                Tok::Punct(";") => {
                    if class_bodies.last() == Some(&(stack.len() - 1)) {
                        self.member_type(k + 1);
                    }
                }
                Tok::Ident => {
                    let l = self.lower(k);
                    if matches!(l, "class" | "interface" | "trait" | "enum") && !self.word_is_name(k) {
                        pending_class = true;
                        // `enum Suit: string`
                        if l == "enum" && k + 2 < n && self.is_word(k + 1) && self.text(k + 2) == ":" {
                            self.role[k + 2] = Role::EnumColon;
                            self.mark_type(k + 3);
                        }
                    }
                }
                _ => {}
            }
            if self.role[k] == Role::Plain {
                // after a parameter type, `&` and `...` belong to the parameter
                let after_operand = prev.is_some_and(|p| self.ends_operand(p) && self.role[p] != Role::Type);
                let role = match kind {
                    Tok::Punct("++" | "--") => {
                        if after_operand {
                            Role::Postfix
                        } else {
                            Role::Unary
                        }
                    }
                    Tok::Punct("-" | "+" | "&") => {
                        if after_operand {
                            Role::Binary
                        } else {
                            Role::Unary
                        }
                    }
                    Tok::Punct("!" | "~" | "@" | "...") => Role::Unary,
                    Tok::Punct("?") => {
                        let nullable = prev.is_none_or(|p| {
                            matches!(self.text(p), "(" | "," | ":")
                                || self.is_word(p) && MODIFIERS.contains(&self.lower(p))
                        });
                        if nullable {
                            Role::Type
                        } else {
                            stack.last_mut().expect("root").ternary += 1;
                            Role::TernaryQuest
                        }
                    }
                    Tok::Punct(":") => {
                        let top = stack.last_mut().expect("root");
                        let named = prev
                            .is_some_and(|p| self.is_word(p) && p > 0 && matches!(self.text(p - 1), "(" | ","))
                            && matches!(top.ctx, Ctx::Call | Ctx::Attr);
                        if named {
                            Role::NamedArgColon
                        } else if top.ternary > 0 {
                            top.ternary -= 1;
                            Role::TernaryColon
                        } else {
                            Role::LabelColon
                        }
                    }
                    _ => Role::Plain,
                };
                self.role[k] = role;
            }
            k += 1;
        }
    }

    /// A parameter starts at `k`: mark its type.
    fn param_type(&mut self, k: usize) {
        let k = self.skip_attrs_and_modifiers(k);
        if k < self.sig.len() && !matches!(self.kind(k), Tok::Variable | Tok::Punct("&" | "..." | ")")) {
            self.mark_type(k);
        }
    }

    /// A class member starts at `k`: mark a property type (`public ?Foo $x`).
    fn member_type(&mut self, k: usize) {
        let start = self.skip_attrs_and_modifiers(k);
        if start == k || start >= self.sig.len() {
            return;
        }
        let first = self.lower(start);
        if matches!(first, "function" | "const" | "fn" | "case" | "use" | "class") {
            return;
        }
        if self.kind(start) != Tok::Variable {
            self.mark_type(start);
        }
    }

    fn within_parens(&self, ctx: Ctx, empty: bool) -> bool {
        let o = self.o;
        match ctx {
            Ctx::Call => {
                if empty {
                    o.SPACE_WITHIN_EMPTY_METHOD_CALL_PARENTHESES
                } else {
                    o.SPACE_WITHIN_METHOD_CALL_PARENTHESES
                }
            }
            Ctx::Decl | Ctx::ClosureDecl => {
                if empty {
                    o.SPACE_WITHIN_EMPTY_METHOD_PARENTHESES
                } else {
                    o.SPACE_WITHIN_METHOD_PARENTHESES
                }
            }
            Ctx::If => o.SPACE_WITHIN_IF_PARENTHESES,
            Ctx::While => o.SPACE_WITHIN_WHILE_PARENTHESES,
            Ctx::For => o.SPACE_WITHIN_FOR_PARENTHESES,
            Ctx::Switch => o.SPACE_WITHIN_SWITCH_PARENTHESES,
            Ctx::Catch => o.SPACE_WITHIN_CATCH_PARENTHESES,
            Ctx::ArrayCall | Ctx::ArrayLit => {
                if empty {
                    o.SPACE_WITHIN_EMPTY_ARRAY_INITIALIZER_BRACES
                } else {
                    o.SPACE_WITHIN_ARRAY_INITIALIZER_BRACES
                }
            }
            Ctx::Index => o.SPACE_WITHIN_BRACKETS,
            _ => o.SPACE_WITHIN_PARENTHESES,
        }
    }

    /// Space in front of the `(` at `b`, which follows `a`.
    fn before_paren(&self, a: usize, b: usize) -> Option<Want> {
        let o = self.o;
        let ctx = self.ctx[b]?;
        let lower = self.lower(a);
        Some(match ctx {
            Ctx::If => space(o.SPACE_BEFORE_IF_PARENTHESES),
            Ctx::While => space(o.SPACE_BEFORE_WHILE_PARENTHESES),
            Ctx::For => space(o.SPACE_BEFORE_FOR_PARENTHESES),
            Ctx::Switch => space(o.SPACE_BEFORE_SWITCH_PARENTHESES),
            Ctx::Catch => space(o.SPACE_BEFORE_CATCH_PARENTHESES),
            Ctx::Decl => space(o.SPACE_BEFORE_METHOD_PARENTHESES),
            Ctx::ClosureDecl if lower == "fn" || (self.text(a) == "&" && self.lower(a - 1) == "fn") => {
                space(o.SPACE_BEFORE_SHORT_CLOSURE_LEFT_PARENTHESIS)
            }
            Ctx::ClosureDecl => space(o.SPACE_BEFORE_CLOSURE_LEFT_PARENTHESIS),
            Ctx::Call | Ctx::ArrayCall => space(o.SPACE_BEFORE_METHOD_CALL_PARENTHESES),
            // PhpStorm leaves `declare (` / `declare(` as written
            Ctx::Declare => Want::AsIs,
            Ctx::ClosureUse => Want::One,
            Ctx::Group if lower == "return" => Want::One,
            _ => return None,
        })
    }

    /// Space in front of a block `{` after statement keyword `kw`.
    fn before_lbrace(&self, kw: &str) -> bool {
        let o = self.o;
        match kw {
            "class" | "interface" | "trait" | "enum" => o.SPACE_BEFORE_CLASS_LBRACE,
            "function" | "fn" => o.SPACE_BEFORE_METHOD_LBRACE,
            "else" => o.SPACE_BEFORE_ELSE_LBRACE,
            "while" => o.SPACE_BEFORE_WHILE_LBRACE,
            "for" | "foreach" => o.SPACE_BEFORE_FOR_LBRACE,
            "do" => o.SPACE_BEFORE_DO_LBRACE,
            "switch" => o.SPACE_BEFORE_SWITCH_LBRACE,
            "try" => o.SPACE_BEFORE_TRY_LBRACE,
            "catch" => o.SPACE_BEFORE_CATCH_LBRACE,
            "finally" => o.SPACE_BEFORE_FINALLY_LBRACE,
            _ => o.SPACE_BEFORE_IF_LBRACE,
        }
    }

    fn binary_space(&self, op: &str) -> Option<bool> {
        let o = self.o;
        Some(match op {
            "=" | "+=" | "-=" | "*=" | "/=" | ".=" | "%=" | "**=" | "??=" | "&=" | "|=" | "^=" | "<<=" | ">>="
            | "=>" => o.SPACE_AROUND_ASSIGNMENT_OPERATORS,
            "&&" | "||" => o.SPACE_AROUND_LOGICAL_OPERATORS,
            "==" | "!=" | "===" | "!==" | "<>" | "<=>" => o.SPACE_AROUND_EQUALITY_OPERATORS,
            "<" | ">" | "<=" | ">=" => o.SPACE_AROUND_RELATIONAL_OPERATORS,
            "&" | "|" | "^" => o.SPACE_AROUND_BITWISE_OPERATORS,
            "+" | "-" => o.SPACE_AROUND_ADDITIVE_OPERATORS,
            "*" | "/" | "%" | "**" => o.SPACE_AROUND_MULTIPLICATIVE_OPERATORS,
            "<<" | ">>" => o.SPACE_AROUND_SHIFT_OPERATORS,
            "." => o.CONCAT_SPACES,
            "??" => o.SPACES_AROUND_NULL_COALESCE_OPERATOR,
            _ => return None,
        })
    }

    /// Space of a binary operator at `k` (None: `k` is not one).
    fn operator_space(&self, k: usize, stack_declare: bool) -> Option<bool> {
        let t = self.text(k);
        match self.kind(k) {
            Tok::Punct("=") if stack_declare => Some(self.o.SPACE_AROUND_ASSIGNMENT_IN_DECLARE),
            Tok::Punct("+" | "-" | "&") if self.role[k] != Role::Binary => None,
            Tok::Punct("|") if self.role[k] == Role::Type => None,
            Tok::Punct(_) => self.binary_space(t),
            _ => None,
        }
    }

    /// The decision for the gap between significant tokens `a` and `b = a + 1`.
    fn decide(&self, a: usize, b: usize) -> Want {
        let declare = self.encl[a] == Ctx::Declare;
        let o = self.o;
        let (ta, tb) = (self.text(a), self.text(b));
        let (ra, rb) = (self.role[a], self.role[b]);
        let (ka, kb) = (self.kind(a), self.kind(b));

        // types
        if ra == Role::Type || rb == Role::Type {
            let compound = |k: usize, t: &str| matches!(t, "|" | "&") && self.role[k] == Role::Type;
            if compound(a, ta) || compound(b, tb) {
                let t = if compound(a, ta) { ta } else { tb };
                return space(if t == "|" {
                    o.SPACES_AROUND_PIPE_IN_UNION_TYPE
                } else {
                    o.SPACES_AROUND_AMPERSAND_IN_INTERSECTION_TYPE
                });
            }
            if ra == Role::Type && ta == "?" {
                return Want::None;
            }
            if ra == Role::Type && ta == "(" || rb == Role::Type && tb == ")" {
                return space(o.SPACES_WITHIN_COMPOUND_TYPE_PARENS);
            }
        }
        // `?:`
        if ra == Role::TernaryQuest && rb == Role::TernaryColon {
            return space(o.SPACE_BETWEEN_TERNARY_QUEST_AND_COLON);
        }
        // commas and semicolons
        if tb == "," && ta != "," {
            return space(o.SPACE_BEFORE_COMMA);
        }
        // `for (;;)` becomes `for (; ;)` like PhpStorm does
        if tb == ";" && !(ta == ";" && self.encl[a] == Ctx::For) {
            return space(o.SPACE_BEFORE_SEMICOLON);
        }
        let closer = matches!(kb, Tok::Punct(")" | "]"));
        if ta == "," {
            // a trailing comma; `list($a, , $b)` does get the space
            return if closer { Want::None } else { space(o.SPACE_AFTER_COMMA) };
        }
        if ta == ";" {
            let in_for = self.encl[a] == Ctx::For;
            return if !in_for {
                Want::Keep
            } else if closer {
                Want::None
            } else {
                space(o.SPACE_AFTER_SEMICOLON)
            };
        }
        // member access
        if matches!(ta, "->" | "?->" | "::") || matches!(tb, "->" | "?->" | "::") {
            return space(o.SPACES_AROUND_ARROW);
        }
        // inside brackets
        if matches!(ka, Tok::Punct("(" | "[")) {
            let ctx = self.ctx[a].unwrap_or(Ctx::Group);
            let empty = matches!(kb, Tok::Punct(")" | "]")) && self.ctx[b] == Some(ctx);
            if ctx == Ctx::Index && kb == Tok::Variable && self.text(b + 1) == "]" {
                return space(o.SPACES_AROUND_VAR_WITHIN_BRACKETS);
            }
            return space(self.within_parens(ctx, empty));
        }
        if matches!(kb, Tok::Punct(")" | "]")) {
            let ctx = self.ctx[b].unwrap_or(Ctx::Group);
            if ctx == Ctx::Index && ka == Tok::Variable && a > 0 && self.text(a - 1) == "[" {
                return space(o.SPACES_AROUND_VAR_WITHIN_BRACKETS);
            }
            return space(self.within_parens(ctx, false));
        }
        if ta == "#[" {
            return Want::None;
        }
        // casts
        if matches!(ka, Tok::Cast(_)) {
            return space(o.SPACE_AFTER_TYPE_CAST);
        }
        // ternary and other colons
        match rb {
            Role::TernaryQuest => return space(o.SPACE_BEFORE_QUEST),
            Role::TernaryColon => return space(o.SPACE_BEFORE_COLON),
            Role::ReturnColon => return space(o.SPACE_BEFORE_COLON_IN_RETURN_TYPE),
            Role::NamedArgColon => return space(o.SPACE_BEFORE_COLON_IN_NAMED_ARGUMENT),
            Role::EnumColon => return space(o.SPACE_BEFORE_COLON_IN_ENUM_BACKED_TYPE),
            // `case 'a' :` is left as written
            Role::LabelColon => return Want::AsIs,
            _ => {}
        }
        match ra {
            Role::TernaryQuest => return space(o.SPACE_AFTER_QUEST),
            Role::TernaryColon => return space(o.SPACE_AFTER_COLON),
            Role::ReturnColon => return space(o.SPACE_AFTER_COLON_IN_RETURN_TYPE),
            Role::NamedArgColon => return space(o.SPACE_AFTER_COLON_IN_NAMED_ARGUMENT),
            Role::EnumColon => return space(o.SPACE_AFTER_COLON_IN_ENUM_BACKED_TYPE),
            _ => {}
        }
        // assignment by reference: `=&` and `= &` are both left as written
        let by_ref = |x: usize, y: usize| self.text(x) == "=" && self.text(y) == "&" && self.role[y] == Role::Unary;
        if by_ref(a, b) || a > 0 && by_ref(a - 1, a) {
            return Want::AsIs;
        }
        // binary operators
        if let Some(on) = self.operator_space(a, declare) {
            return space(on);
        }
        if let Some(on) = self.operator_space(b, declare) {
            return space(on);
        }
        // unary operators
        if ra == Role::Unary {
            return space(if ta == "!" { o.SPACE_AFTER_UNARY_NOT } else { o.SPACE_AROUND_UNARY_OPERATOR });
        }
        if rb == Role::Postfix {
            return space(o.SPACE_AROUND_UNARY_OPERATOR);
        }
        if rb == Role::Unary && tb == "!" && matches!(ka, Tok::Punct(_)) {
            return space(o.SPACE_BEFORE_UNARY_NOT);
        }
        // `(` after a name / keyword
        if tb == "(" {
            if let Some(w) = self.before_paren(a, b) {
                return w;
            }
        }
        // `[` of an index
        if tb == "[" && self.ctx[b] == Some(Ctx::Index) {
            return Want::None;
        }
        // an empty block: `{}`
        if ta == "{" && tb == "}" && self.ctx[a] == Some(Ctx::Block) {
            return Want::None;
        }
        // `{` of a block
        if tb == "{" {
            return match self.ctx[b] {
                Some(Ctx::BraceExpr) => Want::Keep,
                _ => space(self.before_lbrace(&self.block_keyword(b))),
            };
        }
        // `function () use ($x)`
        if ta == ")" && self.is_word(b) && self.lower(b) == "use" {
            return Want::One;
        }
        // `} else`, `} catch`, `} finally`, `} while`
        if ta == "}" && self.ctx[a] == Some(Ctx::Block) && self.is_word(b) {
            match self.lower(b) {
                "else" | "elseif" => return space(o.SPACE_BEFORE_ELSE_KEYWORD),
                "catch" => return space(o.SPACE_BEFORE_CATCH_KEYWORD),
                "finally" => return space(o.SPACE_BEFORE_FINALLY_KEYWORD),
                "while" => return space(o.SPACE_BEFORE_WHILE_KEYWORD),
                _ => {}
            }
        }
        Want::Keep
    }

    /// The statement keyword a block `{` at `k` belongs to (`if`, `function`, `class` …).
    fn block_keyword(&self, k: usize) -> String {
        let mut depth = 0i32;
        for j in (0..k).rev() {
            match self.kind(j) {
                Tok::Punct(")" | "]") => depth += 1,
                Tok::Punct("(" | "[" | "#[") => depth -= 1,
                Tok::Punct(";" | "{" | "}") if depth == 0 => break,
                Tok::Ident if depth == 0 && !self.word_is_name(j) => {
                    let l = self.lower(j);
                    if matches!(
                        l,
                        "class"
                            | "interface"
                            | "trait"
                            | "enum"
                            | "function"
                            | "fn"
                            | "if"
                            | "elseif"
                            | "else"
                            | "while"
                            | "do"
                            | "for"
                            | "foreach"
                            | "switch"
                            | "try"
                            | "catch"
                            | "finally"
                            | "match"
                    ) {
                        return l.to_string();
                    }
                }
                _ => {}
            }
        }
        String::new()
    }

    /// New text for a keyword / constant whose case should change.
    fn recase(&self, k: usize) -> Option<String> {
        if self.kind(k) != Tok::Ident || self.word_is_name(k) {
            return None;
        }
        let next = self.sig.get(k + 1).map(|_| self.text(k + 1));
        let t = self.text(k);
        let l = t.to_ascii_lowercase();
        let o = self.o;
        let prev = k.checked_sub(1).map(|p| self.lower(p)).unwrap_or("");
        // names: `const NEW = 1`, `new Match`, `class List`, `case Default;` in an enum
        if matches!(
            prev,
            "const" | "new" | "class" | "interface" | "trait" | "enum" | "extends" | "implements" | "goto" | "namespace"
        ) || self.role.get(k + 1) == Some(&Role::NamedArgColon)
            // a goto label `list:`, an enum case `case Default;`
            || self.role.get(k + 1) == Some(&Role::LabelColon) && matches!(prev, "" | ";" | "{" | "}")
            || prev == "case" && matches!(next, Some(";" | "="))
        {
            return None;
        }
        let want = match l.as_str() {
            "true" | "false" => {
                if o.LOWER_CASE_BOOLEAN_CONST {
                    l.clone()
                } else if o.UPPER_CASE_BOOLEAN_CONST {
                    t.to_ascii_uppercase()
                } else {
                    return None;
                }
            }
            "null" => {
                if o.LOWER_CASE_NULL_CONST {
                    l.clone()
                } else if o.UPPER_CASE_NULL_CONST {
                    t.to_ascii_uppercase()
                } else {
                    return None;
                }
            }
            w if o.LOWER_CASE_KEYWORDS && KEYWORDS.contains(&w) => l.clone(),
            _ => return None,
        };
        (want != t).then_some(want)
    }
}

/// Can the gap between `a` and `b` be removed without the two tokens running together?
fn can_join(a: &str, b: &str) -> bool {
    let joined = format!("<?php {a}{b}");
    let toks = lex_with_trivia(&joined);
    let sig: Vec<&Token> = toks.iter().filter(|t| !matches!(t.kind, Tok::OpenTag | Tok::Eof)).collect();
    sig.len() == 2 && sig[0].span.text(&joined) == a && sig[1].span.text(&joined) == b
}

fn spacer<'a>(src: &'a str, toks: &'a [Token], o: &'a FormatOptions) -> Spacer<'a> {
    let sig: Vec<usize> = toks
        .iter()
        .enumerate()
        .filter(|(_, t)| !matches!(t.kind, Tok::Comment | Tok::OpenTag | Tok::Eof))
        .map(|(i, _)| i)
        .collect();
    let n = sig.len();
    let low = sig
        .iter()
        .map(|&i| {
            let t = toks[i].span.text(src);
            (toks[i].kind == Tok::Ident && t.bytes().any(|b| b.is_ascii_uppercase())).then(|| t.to_ascii_lowercase())
        })
        .collect();
    let mut s =
        Spacer { src, toks, o, sig, role: vec![Role::Plain; n], ctx: vec![None; n], encl: vec![Ctx::Root; n], low };
    s.analyze();
    s
}

/// What every bracket belongs to: (token index, context) for each `(`, `[`, `#[`, `{` and its
/// closer, in token order.
pub(crate) fn bracket_contexts(src: &str, toks: &[Token], o: &FormatOptions) -> Vec<(usize, Ctx)> {
    let s = spacer(src, toks, o);
    (0..s.sig.len()).filter_map(|k| Some((s.sig[k], s.ctx[k]?))).collect()
}

/// Whitespace and letter-case edits for the whole file, sorted by position.
pub(crate) fn spacing(src: &str, toks: &[Token], o: &FormatOptions) -> Vec<Edit> {
    let s = spacer(src, toks, o);
    let n = s.sig.len();

    let mut edits = Vec::new();
    trailing_comment_tabs(src, toks, o, &mut edits);
    // inside `<?= … ?>`
    let mut in_echo = false;
    for k in 0..n {
        if let Some(text) = s.recase(k) {
            let sp = toks[s.sig[k]].span;
            edits.push(Edit { start: sp.start as usize, end: sp.end as usize, text });
        }
        if k + 1 >= n {
            break;
        }
        let (i, j) = (s.sig[k], s.sig[k + 1]);
        // `<?= $x ?>`: one space inside short echo tags (SPACES_WITHIN_SHORT_ECHO_TAGS)
        if j == i + 1 && (toks[i].kind == Tok::OpenTagEcho || toks[j].kind == Tok::CloseTag && in_echo) {
            let (start, end) = (toks[i].span.end as usize, toks[j].span.start as usize);
            let gap = &src[start..end];
            let want = if o.SPACES_WITHIN_SHORT_ECHO_TAGS { " " } else { "" };
            if !gap.contains('\n')
                && gap != want
                && !(want.is_empty() && toks[j].kind == Tok::CloseTag && gap.is_empty())
            {
                edits.push(Edit { start, end, text: want.into() });
            }
        }
        match toks[i].kind {
            Tok::OpenTagEcho => in_echo = true,
            Tok::CloseTag => in_echo = false,
            _ => {}
        }
        // only gaps between adjacent tokens (no comment between) on one line, both PHP code
        if j != i + 1 || matches!(toks[i].kind, Tok::InlineHtml | Tok::CloseTag | Tok::OpenTagEcho) {
            continue;
        }
        if matches!(toks[j].kind, Tok::InlineHtml | Tok::CloseTag) {
            continue;
        }
        let (start, end) = (toks[i].span.end as usize, toks[j].span.start as usize);
        let gap = &src[start..end];
        if gap.contains('\n') || gap.contains('\r') {
            continue;
        }
        let want = match s.decide(k, k + 1) {
            Want::Keep if gap.is_empty() => continue,
            Want::Keep | Want::One => " ",
            Want::None => "",
            Want::AsIs => {
                if let Some(text) = untab(gap, o) {
                    edits.push(Edit { start, end, text });
                }
                continue;
            }
        };
        if gap == want {
            continue;
        }
        if want.is_empty() && !can_join(toks[i].span.text(src), toks[j].span.text(src)) {
            if gap != " " {
                edits.push(Edit { start, end, text: " ".into() });
            }
            continue;
        }
        edits.push(Edit { start, end, text: want.into() });
    }
    edits.sort_by_key(|e| e.start);
    edits
}

/// Tabs in a gap that is otherwise kept become `TAB_SIZE` spaces each (as PhpStorm does), unless
/// indenting with tabs. None when nothing changes.
fn untab(gap: &str, o: &FormatOptions) -> Option<String> {
    (!o.USE_TAB_CHARACTER && gap.contains('\t')).then(|| gap.replace('\t', &" ".repeat(o.TAB_SIZE)))
}

/// Tabs in front of a trailing comment: `$a = 1;\t\t// x`.
fn trailing_comment_tabs(src: &str, toks: &[Token], o: &FormatOptions, edits: &mut Vec<Edit>) {
    for w in toks.windows(2) {
        let (a, c) = (&w[0], &w[1]);
        if c.kind != Tok::Comment || matches!(a.kind, Tok::Comment | Tok::InlineHtml | Tok::Eof) {
            continue;
        }
        let (start, end) = (a.span.end as usize, c.span.start as usize);
        let gap = &src[start..end];
        if gap.contains('\n') {
            continue;
        }
        if let Some(text) = untab(gap, o) {
            edits.push(Edit { start, end, text });
        }
    }
}
