//! Expressions: precedence climbing over unary / postfix / primary expressions.

use super::{Parser, MAX_DEPTH};
use crate::ast::*;
use crate::lexer::{Tok, Token};
use crate::span::Span;

fn binop(tok: &Tok, text: &str) -> Option<(&'static str, u8, bool)> {
    // (op, left binding power, right associative)
    match tok {
        Tok::Punct(p) => Some(match *p {
            "||" => ("||", 7, false),
            "&&" => ("&&", 8, false),
            "|" => ("|", 9, false),
            "^" => ("^", 10, false),
            "&" => ("&", 11, false),
            "==" => ("==", 12, false),
            "!=" => ("!=", 12, false),
            "<>" => ("!=", 12, false),
            "===" => ("===", 12, false),
            "!==" => ("!==", 12, false),
            "<=>" => ("<=>", 12, false),
            "<" => ("<", 13, false),
            "<=" => ("<=", 13, false),
            ">" => (">", 13, false),
            ">=" => (">=", 13, false),
            "." => (".", 14, false),
            "<<" => ("<<", 15, false),
            ">>" => (">>", 15, false),
            "+" => ("+", 16, false),
            "-" => ("-", 16, false),
            "*" => ("*", 17, false),
            "/" => ("/", 17, false),
            "%" => ("%", 17, false),
            "**" => ("**", 21, true),
            "??" => ("??", 6, true),
            _ => return None,
        }),
        Tok::Ident => {
            if text.eq_ignore_ascii_case("or") {
                Some(("or", 1, false))
            } else if text.eq_ignore_ascii_case("xor") {
                Some(("xor", 2, false))
            } else if text.eq_ignore_ascii_case("and") {
                Some(("and", 3, false))
            } else {
                None
            }
        }
        _ => None,
    }
}

const ASSIGN_OPS: &[&str] = &["=", "+=", "-=", "*=", "/=", ".=", "%=", "&=", "|=", "^=", "<<=", ">>=", "**=", "??="];

impl Parser<'_> {
    pub(super) fn parse_expr(&mut self) -> Expr {
        self.parse_expr_bp(0)
    }

    pub(super) fn parse_expr_bp(&mut self, min: u8) -> Expr {
        if self.depth > MAX_DEPTH {
            return self.err_expr();
        }
        self.depth += 1;
        let r = self.parse_expr_bp_inner(min);
        self.depth -= 1;
        r
    }

    fn parse_expr_bp_inner(&mut self, min: u8) -> Expr {
        let start = self.span().start as usize;
        let mut lhs = self.parse_unary();
        loop {
            let t = self.tok();
            // ternary
            if matches!(t.kind, Tok::Punct("?")) {
                if 5 < min {
                    break;
                }
                self.bump();
                let then = if self.at_punct(":") { None } else { Some(Box::new(self.parse_expr_bp(5))) };
                self.expect_punct(":");
                let els = self.parse_expr_bp(6);
                lhs = self.mk_expr(ExprKind::Ternary { cond: Box::new(lhs), then, els: Box::new(els) }, start);
                continue;
            }
            if self.kind() == Tok::Ident && self.text().eq_ignore_ascii_case("instanceof") {
                if 19 < min {
                    break;
                }
                self.bump();
                let class = self.parse_class_ref_operand();
                lhs = self.mk_expr(ExprKind::InstanceOf { expr: Box::new(lhs), class }, start);
                continue;
            }
            let Some((op, lbp, right)) = binop(&t.kind, self.text_of(t)) else { break };
            if lbp < min {
                break;
            }
            self.bump();
            let rhs = self.parse_expr_bp(if right { lbp } else { lbp + 1 });
            lhs = self.mk_expr(ExprKind::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs) }, start);
        }
        lhs
    }

    fn parse_class_ref_operand(&mut self) -> ClassRef {
        let save = self.pos;
        if let Some(n) = self.name_here() {
            if self.at_punct("::") {
                // `$x instanceof static::$class`: a full expression, not a class name
                self.pos = save;
                let e = self.parse_unary();
                return ClassRef::Expr(Box::new(e));
            }
            ClassRef::Name(n)
        } else if self.at_punct("(") {
            self.bump();
            let e = self.parse_expr();
            self.expect_punct(")");
            ClassRef::Expr(Box::new(e))
        } else {
            let e = self.parse_unary();
            ClassRef::Expr(Box::new(e))
        }
    }

    fn parse_unary(&mut self) -> Expr {
        let t = self.tok();
        let start = t.span.start as usize;
        match t.kind {
            Tok::Punct(op @ ("!" | "-" | "+" | "~" | "@" | "++" | "--" | "&")) => {
                self.bump();
                let bp = if op == "!" { 18 } else { 20 };
                let e = self.parse_expr_bp(bp);
                self.mk_expr(ExprKind::Unary { op, expr: Box::new(e) }, start)
            }
            Tok::Cast(ty) => {
                self.bump();
                let e = self.parse_expr_bp(20);
                self.mk_expr(ExprKind::Cast { ty, expr: Box::new(e) }, start)
            }
            Tok::Ident => {
                let w = self.text().to_ascii_lowercase();
                match w.as_str() {
                    "new" => {
                        let e = self.parse_new();
                        self.parse_postfix(e, start)
                    }
                    "clone" => {
                        self.bump();
                        let e = self.parse_expr_bp(20);
                        self.mk_expr(ExprKind::Clone(Box::new(e)), start)
                    }
                    "print" => {
                        self.bump();
                        let e = self.parse_expr_bp(4);
                        self.mk_expr(ExprKind::Print(Box::new(e)), start)
                    }
                    "throw" => {
                        self.bump();
                        let e = self.parse_expr_bp(0);
                        self.mk_expr(ExprKind::Throw(Box::new(e)), start)
                    }
                    "include" | "include_once" | "require" | "require_once" => {
                        self.bump();
                        let e = self.parse_expr_bp(4);
                        self.mk_expr(ExprKind::Include(Box::new(e)), start)
                    }
                    "yield" => {
                        self.bump();
                        if self.eat_kw("from") {
                            let e = self.parse_expr_bp(4);
                            return self.mk_expr(ExprKind::YieldFrom(Box::new(e)), start);
                        }
                        let can_start =
                            !matches!(self.kind(), Tok::Punct(";" | ")" | "," | "]" | "}") | Tok::Eof | Tok::CloseTag);
                        if !can_start {
                            return self.mk_expr(ExprKind::Yield { key: None, value: None }, start);
                        }
                        let v = self.parse_expr_bp(4);
                        if self.eat_punct("=>") {
                            let v2 = self.parse_expr_bp(4);
                            self.mk_expr(ExprKind::Yield { key: Some(Box::new(v)), value: Some(Box::new(v2)) }, start)
                        } else {
                            self.mk_expr(ExprKind::Yield { key: None, value: Some(Box::new(v)) }, start)
                        }
                    }
                    _ => {
                        let p = self.parse_primary();
                        self.parse_postfix(p, start)
                    }
                }
            }
            _ => {
                let p = self.parse_primary();
                self.parse_postfix(p, start)
            }
        }
    }

    fn is_lvalue(e: &Expr) -> bool {
        matches!(
            e.kind,
            ExprKind::Var(_)
                | ExprKind::VarVar(_)
                | ExprKind::Prop { .. }
                | ExprKind::StaticProp { .. }
                | ExprKind::Index { .. }
                | ExprKind::Array(_)
                | ExprKind::Call { .. }
        )
    }

    fn parse_postfix(&mut self, mut e: Expr, start: usize) -> Expr {
        loop {
            let t = self.tok();
            match t.kind {
                Tok::Punct("[") => {
                    self.bump();
                    let idx = if self.at_punct("]") { None } else { Some(Box::new(self.parse_expr())) };
                    self.expect_punct("]");
                    e = self.mk_expr(ExprKind::Index { base: Box::new(e), index: idx }, start);
                }
                Tok::Punct(op @ ("->" | "?->")) => {
                    self.bump();
                    let nullsafe = op == "?->";
                    let name = self.parse_member_name();
                    if self.at_punct("(") {
                        let args = self.parse_args();
                        e = self.mk_expr(ExprKind::MethodCall { obj: Box::new(e), name, args, nullsafe }, start);
                    } else {
                        e = self.mk_expr(ExprKind::Prop { obj: Box::new(e), name, nullsafe }, start);
                    }
                }
                Tok::Punct("{")
                    if t.span.start as usize == self.prev_end()
                        && matches!(e.kind, ExprKind::Var(_) | ExprKind::Index { .. } | ExprKind::Prop { .. }) =>
                {
                    // PHP 4 style string offset: $str{0}
                    self.bump();
                    let idx = self.parse_expr();
                    self.expect_punct("}");
                    e = self.mk_expr(ExprKind::Index { base: Box::new(e), index: Some(Box::new(idx)) }, start);
                }
                Tok::Punct("::") => {
                    self.bump();
                    e = self.parse_static_member(e, start);
                }
                Tok::Punct("(") => {
                    let args = self.parse_args();
                    e = self.mk_expr(ExprKind::Call { callee: Box::new(e), args }, start);
                }
                Tok::Punct(op @ ("++" | "--")) => {
                    self.bump();
                    let op: &'static str = if op == "++" { "p++" } else { "p--" };
                    e = self.mk_expr(ExprKind::Unary { op, expr: Box::new(e) }, start);
                }
                Tok::Punct(op) if ASSIGN_OPS.contains(&op) && Self::is_lvalue(&e) => {
                    self.bump();
                    let by_ref = op == "=" && self.at_punct("&") && {
                        self.bump();
                        true
                    };
                    let value = self.parse_expr_bp(4);
                    e = self
                        .mk_expr(ExprKind::Assign { target: Box::new(e), value: Box::new(value), op, by_ref }, start);
                    return e;
                }
                _ => break,
            }
        }
        e
    }

    /// What follows `Class::`: `$prop`, `CONST`, `method()`, `$$name`, `{expr}()`.
    fn parse_static_member(&mut self, e: Expr, start: usize) -> Expr {
        let class = match e.kind {
            ExprKind::Name(n) => ClassRef::Name(n),
            other => ClassRef::Expr(Box::new(Expr { kind: other, span: e.span })),
        };
        let nt = self.tok();
        match nt.kind {
            Tok::Variable => {
                self.bump();
                let id = Ident { text: self.text_of(nt)[1..].to_string(), span: nt.span };
                if self.at_punct("(") {
                    let args = self.parse_args();
                    let v = Expr { kind: ExprKind::Var(id.text.clone()), span: id.span };
                    self.mk_expr(ExprKind::StaticCall { class, name: MemberName::Expr(Box::new(v)), args }, start)
                } else {
                    self.mk_expr(ExprKind::StaticProp { class, name: MemberName::Ident(id) }, start)
                }
            }
            Tok::Ident => {
                self.bump();
                let id = Ident { text: self.text_of(nt).to_string(), span: nt.span };
                if self.at_punct("(") {
                    let args = self.parse_args();
                    self.mk_expr(ExprKind::StaticCall { class, name: MemberName::Ident(id), args }, start)
                } else {
                    self.mk_expr(ExprKind::ClassConst { class, name: id }, start)
                }
            }
            Tok::Punct("$") => {
                let v = self.parse_primary();
                self.mk_expr(ExprKind::StaticProp { class, name: MemberName::Expr(Box::new(v)) }, start)
            }
            Tok::Punct("{") => {
                self.bump();
                let inner = self.parse_expr();
                self.expect_punct("}");
                let args = if self.at_punct("(") { self.parse_args() } else { Vec::new() };
                self.mk_expr(ExprKind::StaticCall { class, name: MemberName::Expr(Box::new(inner)), args }, start)
            }
            _ => {
                // `Foo::` with nothing valid after (incomplete code while typing)
                self.error_here("expected member name after '::'");
                let p = self.prev_end();
                let id = Ident { text: String::new(), span: Span::new(p, p) };
                self.mk_expr(ExprKind::ClassConst { class, name: id }, start)
            }
        }
    }

    fn parse_member_name(&mut self) -> MemberName {
        let t = self.tok();
        match t.kind {
            Tok::Ident => {
                self.bump();
                MemberName::Ident(Ident { text: self.text_of(t).to_string(), span: t.span })
            }
            Tok::Variable => {
                self.bump();
                MemberName::Expr(Box::new(Expr { kind: ExprKind::Var(self.text_of(t)[1..].to_string()), span: t.span }))
            }
            Tok::Punct("{") => {
                self.bump();
                let e = self.parse_expr();
                self.expect_punct("}");
                MemberName::Expr(Box::new(e))
            }
            _ => {
                // incomplete `$foo->` while typing
                self.error_here("expected member name");
                let p = self.prev_end();
                MemberName::Ident(Ident { text: String::new(), span: Span::new(p, p) })
            }
        }
    }

    pub(super) fn parse_args(&mut self) -> Vec<Arg> {
        let mut args = Vec::new();
        if !self.expect_punct("(") {
            return args;
        }
        loop {
            if self.at_punct(")") || self.at_eof() {
                break;
            }
            if matches!(self.kind(), Tok::Punct(";" | "}")) {
                break;
            }
            let before = self.pos;
            let mut spread = false;
            if self.at_punct("...") {
                self.bump();
                if self.at_punct(")") {
                    break; // first-class callable syntax
                }
                spread = true;
            }
            let mut name = None;
            if self.kind() == Tok::Ident && self.peek_punct(1, ":") && !self.peek_punct(2, ":") {
                name = self.ident_here();
                self.bump();
            }
            let value = self.parse_expr();
            args.push(Arg { name, value, spread });
            if !self.eat_punct(",") {
                break;
            }
            if self.pos == before {
                self.bump();
            }
        }
        self.expect_punct(")");
        args
    }

    fn parse_array_items(&mut self, close: &str) -> Vec<ArrayItem> {
        let mut items = Vec::new();
        loop {
            if self.at_punct(close) || self.at_eof() {
                break;
            }
            if matches!(self.kind(), Tok::Punct(";" | "}")) {
                break;
            }
            if self.eat_punct(",") {
                continue; // skipped element in list()
            }
            let before = self.pos;
            if self.eat_punct("...") {
                let v = self.parse_expr();
                items.push(ArrayItem { key: None, value: v, by_ref: false, spread: true });
            } else {
                let by_ref = self.eat_punct("&");
                let first = self.parse_expr();
                if self.eat_punct("=>") {
                    let by_ref = self.eat_punct("&");
                    let v = self.parse_expr();
                    items.push(ArrayItem { key: Some(first), value: v, by_ref, spread: false });
                } else {
                    items.push(ArrayItem { key: None, value: first, by_ref, spread: false });
                }
            }
            if !self.eat_punct(",") {
                break;
            }
            if self.pos == before {
                self.bump();
            }
        }
        items
    }

    fn parse_new(&mut self) -> Expr {
        let start = self.span().start as usize;
        self.bump(); // new
        let attrs = if self.at_punct("#[") {
            self.attrs.clear();
            self.parse_attributes();
            std::mem::take(&mut self.attrs)
        } else {
            Vec::new()
        };
        if self.at_kw("class") {
            let cstart = self.span().start as usize;
            self.bump();
            let args = if self.at_punct("(") { self.parse_args() } else { Vec::new() };
            let mut decl = self.parse_class_rest(ClassKind::Class, None, Modifiers::default(), None, cstart);
            decl.attrs = attrs;
            return self.mk_expr(ExprKind::New { class: ClassRef::Anon(Box::new(decl)), args }, start);
        }
        let static_prop_start = {
            // `new static::$class(...)` / `new self::$map[$k]`: the class name comes from a static property
            let save = self.pos;
            let ok = self.name_here().is_some() && self.at_punct("::") && self.peek_at(1).kind == Tok::Variable;
            self.pos = save;
            ok
        };
        let class = if static_prop_start {
            let vstart = self.span().start as usize;
            let n = self.name_here().unwrap();
            self.bump();
            let nt = self.bump();
            let id = Ident { text: self.text_of(nt)[1..].to_string(), span: nt.span };
            let mut e =
                self.mk_expr(ExprKind::StaticProp { class: ClassRef::Name(n), name: MemberName::Ident(id) }, vstart);
            while self.at_punct("[") {
                self.bump();
                let idx = if self.at_punct("]") { None } else { Some(Box::new(self.parse_expr())) };
                self.expect_punct("]");
                e = self.mk_expr(ExprKind::Index { base: Box::new(e), index: idx }, vstart);
            }
            ClassRef::Expr(Box::new(e))
        } else if let Some(n) = self.name_here() {
            ClassRef::Name(n)
        } else if self.at_punct("(") {
            self.bump();
            let e = self.parse_expr();
            self.expect_punct(")");
            ClassRef::Expr(Box::new(e))
        } else if self.kind() == Tok::Variable || self.at_punct("$") {
            let vstart = self.span().start as usize;
            let mut e = self.parse_primary();
            // property / static-prop / index chain, no calls
            loop {
                if matches!(self.kind(), Tok::Punct("->" | "?->")) {
                    self.bump();
                    let name = self.parse_member_name();
                    e = self.mk_expr(ExprKind::Prop { obj: Box::new(e), name, nullsafe: false }, vstart);
                } else if self.at_punct("::") && self.peek_at(1).kind == Tok::Variable {
                    self.bump();
                    let nt = self.bump();
                    let id = Ident { text: self.text_of(nt)[1..].to_string(), span: nt.span };
                    let class = ClassRef::Expr(Box::new(e));
                    e = self.mk_expr(ExprKind::StaticProp { class, name: MemberName::Ident(id) }, vstart);
                } else if self.at_punct("[") {
                    self.bump();
                    let idx = if self.at_punct("]") { None } else { Some(Box::new(self.parse_expr())) };
                    self.expect_punct("]");
                    e = self.mk_expr(ExprKind::Index { base: Box::new(e), index: idx }, vstart);
                } else {
                    break;
                }
            }
            ClassRef::Expr(Box::new(e))
        } else {
            self.error_here("expected class name after 'new'");
            return self.mk_expr(ExprKind::Error, start);
        };
        let args = if self.at_punct("(") { self.parse_args() } else { Vec::new() };
        self.mk_expr(ExprKind::New { class, args }, start)
    }

    fn parse_closure(&mut self, start: usize) -> Expr {
        let attrs = std::mem::take(&mut self.attrs);
        let is_static = self.eat_kw("static");
        let doc = None;
        if self.eat_kw("fn") {
            let by_ref = self.eat_punct("&");
            let params = self.parse_params();
            let ret = if self.eat_punct(":") { self.parse_type() } else { None };
            self.expect_punct("=>");
            let body = self.parse_expr_bp(4);
            let decl = FunctionDecl {
                params,
                ret,
                by_ref,
                arrow_body: Some(Box::new(body)),
                is_static,
                attrs,
                doc,
                span: Span::new(start, self.prev_end()),
                ..Default::default()
            };
            return self.mk_expr(ExprKind::Closure(Box::new(decl)), start);
        }
        self.bump(); // function
        let by_ref = self.eat_punct("&");
        let params = self.parse_params();
        let mut uses = Vec::new();
        if self.eat_kw("use") && self.expect_punct("(") {
            loop {
                if self.at_punct(")") || self.at_eof() {
                    break;
                }
                let by_ref = self.eat_punct("&");
                if self.kind() == Tok::Variable {
                    let t = self.bump();
                    uses.push(ClosureUse { name: self.text_of(t)[1..].to_string(), span: t.span, by_ref });
                } else {
                    self.error_here("expected variable");
                    break;
                }
                if !self.eat_punct(",") {
                    break;
                }
            }
            self.expect_punct(")");
        }
        let ret = if self.eat_punct(":") { self.parse_type() } else { None };
        let body = self.parse_braced_block();
        let decl = FunctionDecl {
            params,
            ret,
            by_ref,
            uses,
            body: Some(body),
            is_static,
            attrs,
            doc,
            span: Span::new(start, self.prev_end()),
            ..Default::default()
        };
        self.mk_expr(ExprKind::Closure(Box::new(decl)), start)
    }

    fn parse_primary(&mut self) -> Expr {
        let t = self.tok();
        let start = t.span.start as usize;
        match t.kind {
            Tok::Variable => {
                self.bump();
                self.mk_expr(ExprKind::Var(self.text_of(t)[1..].to_string()), start)
            }
            Tok::Punct("$") => {
                // `$$name` / `${expr}`
                self.bump();
                let inner = if self.at_punct("{") {
                    self.bump();
                    let e = self.parse_expr();
                    self.expect_punct("}");
                    e
                } else {
                    self.parse_primary()
                };
                self.mk_expr(ExprKind::VarVar(Box::new(inner)), start)
            }
            Tok::Int | Tok::Float | Tok::Str => {
                self.bump();
                self.mk_expr(ExprKind::Lit, start)
            }
            Tok::Template => {
                self.bump();
                let parts = self.parse_template(t.span);
                self.mk_expr(ExprKind::Interp(parts), start)
            }
            Tok::Punct("(") => {
                self.bump();
                let e = self.parse_expr();
                self.expect_punct(")");
                self.mk_expr(ExprKind::Paren(Box::new(e)), start)
            }
            Tok::Punct("[") => {
                self.bump();
                let items = self.parse_array_items("]");
                self.expect_punct("]");
                self.mk_expr(ExprKind::Array(items), start)
            }
            Tok::Punct("#[") => {
                self.attrs.clear();
                self.parse_attributes();
                self.parse_primary()
            }
            Tok::Name => {
                self.bump();
                self.mk_expr(ExprKind::Name(Name { text: self.text_of(t).to_string(), span: t.span }), start)
            }
            Tok::Ident => self.parse_word_expr(t, start),
            Tok::OpenTagEcho | Tok::CloseTag | Tok::InlineHtml | Tok::Eof => {
                self.error_here("expected expression");
                self.err_expr()
            }
            Tok::Punct(";" | ")" | "]" | "}" | "," | ":" | "=>") => {
                self.error_here("expected expression");
                self.err_expr()
            }
            _ => {
                self.error_here("unexpected token");
                self.bump();
                self.mk_expr(ExprKind::Error, start)
            }
        }
    }

    /// An expression starting with a bare word: `array(...)`, `isset(...)`, `exit`, closures,
    /// `match`, or a constant / function name.
    fn parse_word_expr(&mut self, t: Token, start: usize) -> Expr {
        let w = self.text_of(t).to_ascii_lowercase();
        match w.as_str() {
            "array" | "list" if self.peek_punct(1, "(") => {
                self.bump();
                self.bump();
                let items = self.parse_array_items(")");
                self.expect_punct(")");
                self.mk_expr(ExprKind::Array(items), start)
            }
            "isset" if self.peek_punct(1, "(") => self.parse_isset(start),
            "unset" if self.peek_punct(1, "(") => {
                self.bump();
                let args = self.parse_args();
                let callee = Expr { kind: ExprKind::Name(Name { text: "unset".into(), span: t.span }), span: t.span };
                self.mk_expr(ExprKind::Call { callee: Box::new(callee), args }, start)
            }
            "empty" if self.peek_punct(1, "(") => {
                self.bump();
                self.bump();
                let e = self.parse_expr();
                self.expect_punct(")");
                self.mk_expr(ExprKind::Empty(Box::new(e)), start)
            }
            "exit" | "die" => {
                self.bump();
                let mut arg = None;
                if self.eat_punct("(") {
                    if !self.at_punct(")") {
                        arg = Some(Box::new(self.parse_expr()));
                    }
                    self.expect_punct(")");
                }
                self.mk_expr(ExprKind::Exit(arg), start)
            }
            "function" | "fn" => self.parse_closure(start),
            "static" if self.peek_kw(1, "function") || self.peek_kw(1, "fn") => self.parse_closure(start),
            "match" if self.peek_punct(1, "(") => self.parse_match(start),
            _ => {
                self.bump();
                self.mk_expr(ExprKind::Name(Name { text: self.text_of(t).to_string(), span: t.span }), start)
            }
        }
    }

    fn parse_isset(&mut self, start: usize) -> Expr {
        self.bump();
        self.bump();
        let mut v = Vec::new();
        while !self.at_punct(")") && !self.at_eof() && !self.at_stmt_end() {
            let before = self.pos;
            v.push(self.parse_expr());
            if !self.eat_punct(",") {
                break;
            }
            if self.pos == before {
                self.bump();
            }
        }
        self.expect_punct(")");
        self.mk_expr(ExprKind::Isset(v), start)
    }

    fn parse_match(&mut self, start: usize) -> Expr {
        self.bump(); // match
        self.bump(); // (
        let subject = self.parse_expr();
        self.expect_punct(")");
        let mut arms = Vec::new();
        if self.expect_punct("{") {
            loop {
                if self.at_punct("}") || self.at_eof() {
                    break;
                }
                let before = self.pos;
                let mut conds = Vec::new();
                if self.at_kw("default") && matches!(self.peek_at(1).kind, Tok::Punct("=>" | ",")) {
                    self.bump();
                    self.eat_punct(",");
                } else {
                    loop {
                        if self.at_punct("=>") || self.at_eof() {
                            break;
                        }
                        conds.push(self.parse_expr());
                        if !self.eat_punct(",") {
                            break;
                        }
                    }
                }
                self.expect_punct("=>");
                let body = self.parse_expr();
                arms.push(MatchArm { conds, body });
                if !self.eat_punct(",") {
                    break;
                }
                if self.pos == before {
                    self.bump();
                }
            }
            self.expect_punct("}");
        }
        self.mk_expr(ExprKind::Match { subject: Box::new(subject), arms }, start)
    }
}
