//! Error-tolerant recursive descent parser. Never panics, always makes progress.
//!
//! - `mod.rs`: the parser state and token helpers
//! - `stmt.rs`: statements (`if`, `foreach`, `switch`, `try`, `namespace`, `use` …)
//! - `decl.rs`: functions, classes, members, parameters, types and attributes
//! - `expr.rs`: expressions (operators, calls, `new`, closures, `match` …)
//! - `template.rs`: variables inside double-quoted strings and heredocs

mod decl;
mod expr;
mod stmt;
mod template;

use crate::ast::*;
use crate::lexer::{self, Tok, Token};
use crate::span::Span;

#[derive(Debug, Clone)]
pub struct ParseError {
    pub message: String,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Parse {
    pub stmts: Vec<Stmt>,
    pub errors: Vec<ParseError>,
}

pub fn parse(src: &str) -> Parse {
    parse_tokens(src, lexer::lex(src))
}

/// Parse `src` as PHP code from the first byte, without a `<?php` tag (Blade templates turned
/// into code by [`crate::blade::to_php`]).
pub fn parse_code(src: &str) -> Parse {
    parse_tokens(src, lexer::lex_php_range(src, 0, src.len()))
}

/// [`parse_code`] for Blade code, [`parse`] for everything else.
pub fn parse_for(src: &str, blade: bool) -> Parse {
    if blade {
        parse_code(src)
    } else {
        parse(src)
    }
}

fn parse_tokens(src: &str, toks: Vec<Token>) -> Parse {
    let mut p = Parser::new(src, toks);
    let mut stmts = Vec::new();
    while !p.at_eof() {
        let before = p.pos;
        stmts.push(p.parse_stmt());
        if p.pos == before {
            p.error_here("unexpected token");
            p.pos += 1;
        }
    }
    Parse { stmts, errors: p.errors }
}

const MAX_DEPTH: u32 = 200;

struct Parser<'a> {
    src: &'a str,
    toks: Vec<Token>,
    pos: usize,
    errors: Vec<ParseError>,
    attrs: Vec<Expr>,
    extra_members: Vec<Member>,
    depth: u32,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str, toks: Vec<Token>) -> Self {
        Parser { src, toks, pos: 0, errors: Vec::new(), attrs: Vec::new(), extra_members: Vec::new(), depth: 0 }
    }

    // ---- token helpers -------------------------------------------------

    fn tok(&self) -> Token {
        self.toks[self.pos.min(self.toks.len() - 1)]
    }

    fn peek_at(&self, n: usize) -> Token {
        self.toks[(self.pos + n).min(self.toks.len() - 1)]
    }

    fn kind(&self) -> Tok {
        self.tok().kind
    }

    fn span(&self) -> Span {
        self.tok().span
    }

    fn prev_end(&self) -> usize {
        if self.pos == 0 {
            0
        } else {
            self.toks[(self.pos - 1).min(self.toks.len() - 1)].span.end as usize
        }
    }

    fn text_of(&self, t: Token) -> &'a str {
        t.span.text(self.src)
    }

    fn text(&self) -> &'a str {
        self.text_of(self.tok())
    }

    fn at_eof(&self) -> bool {
        self.kind() == Tok::Eof
    }

    fn bump(&mut self) -> Token {
        let t = self.tok();
        if t.kind != Tok::Eof {
            self.pos += 1;
        }
        t
    }

    fn at_punct(&self, p: &str) -> bool {
        matches!(self.kind(), Tok::Punct(x) if x == p)
    }

    fn peek_punct(&self, n: usize, p: &str) -> bool {
        matches!(self.peek_at(n).kind, Tok::Punct(x) if x == p)
    }

    fn eat_punct(&mut self, p: &str) -> bool {
        if self.at_punct(p) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn at_kw(&self, kw: &str) -> bool {
        self.kind() == Tok::Ident && self.text().eq_ignore_ascii_case(kw)
    }

    fn peek_kw(&self, n: usize, kw: &str) -> bool {
        let t = self.peek_at(n);
        t.kind == Tok::Ident && self.text_of(t).eq_ignore_ascii_case(kw)
    }

    fn eat_kw(&mut self, kw: &str) -> bool {
        if self.at_kw(kw) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn error_at(&mut self, span: Span, msg: impl Into<String>) {
        // avoid error storms: one error per position
        if let Some(last) = self.errors.last() {
            if last.span.start == span.start {
                return;
            }
        }
        self.errors.push(ParseError { message: msg.into(), span });
    }

    fn error_here(&mut self, msg: &str) {
        let t = self.tok();
        let found = match t.kind {
            Tok::Eof => "end of file".to_string(),
            _ => format!("'{}'", self.text_of(t)),
        };
        self.error_at(t.span, format!("Syntax error: {msg}, found {found}"));
    }

    fn expect_punct(&mut self, p: &str) -> bool {
        if self.eat_punct(p) {
            true
        } else {
            let sp = Span::new(self.prev_end(), self.prev_end());
            let t = self.tok();
            let found = if t.kind == Tok::Eof { "end of file".to_string() } else { format!("'{}'", self.text_of(t)) };
            self.error_at(sp, format!("Syntax error: expected '{p}', found {found}"));
            false
        }
    }

    /// `;` or `?>` terminate a statement.
    fn expect_semi(&mut self) {
        if self.eat_punct(";") {
            return;
        }
        if self.kind() == Tok::CloseTag {
            self.pos += 1;
            return;
        }
        if matches!(self.kind(), Tok::Eof | Tok::InlineHtml) {
            return;
        }
        self.expect_punct(";");
    }

    fn at_stmt_end(&self) -> bool {
        self.at_punct(";") || matches!(self.kind(), Tok::CloseTag | Tok::Eof)
    }

    fn ident_here(&mut self) -> Option<Ident> {
        let t = self.tok();
        if t.kind == Tok::Ident {
            self.pos += 1;
            Some(Ident { text: self.text_of(t).to_string(), span: t.span })
        } else {
            None
        }
    }

    fn name_here(&mut self) -> Option<Name> {
        let t = self.tok();
        if matches!(t.kind, Tok::Ident | Tok::Name) {
            self.pos += 1;
            Some(Name { text: self.text_of(t).to_string(), span: t.span })
        } else {
            None
        }
    }

    fn mk_expr(&self, kind: ExprKind, start: usize) -> Expr {
        Expr { kind, span: Span::new(start, self.prev_end().max(start)) }
    }

    fn err_expr(&self) -> Expr {
        let p = self.prev_end();
        Expr { kind: ExprKind::Error, span: Span::new(p, p) }
    }
}
