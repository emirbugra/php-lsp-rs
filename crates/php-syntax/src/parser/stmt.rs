//! Statements.

use super::{Parser, MAX_DEPTH};
use crate::ast::*;
use crate::lexer::Tok;
use crate::span::Span;

impl Parser<'_> {
    pub(super) fn parse_stmt(&mut self) -> Stmt {
        if self.depth > MAX_DEPTH {
            let s = self.bump();
            return Stmt { kind: StmtKind::Nop, span: s.span, doc: None };
        }
        self.depth += 1;
        let r = self.parse_stmt_inner();
        self.depth -= 1;
        r
    }

    fn stmt(&self, kind: StmtKind, start: usize, doc: Option<Span>) -> Stmt {
        Stmt { kind, span: Span::new(start, self.prev_end().max(start)), doc }
    }

    fn parse_stmt_inner(&mut self) -> Stmt {
        // a docblock before the attributes still belongs to the declaration after them
        let lead_doc = self.tok().doc;
        if self.at_punct("#[") {
            self.attrs.clear();
            self.parse_attributes();
        } else {
            self.attrs.clear();
        }
        let t = self.tok();
        let start = t.span.start as usize;
        let doc = t.doc.or(lead_doc);
        match t.kind {
            Tok::InlineHtml => {
                self.bump();
                self.stmt(StmtKind::InlineHtml, start, doc)
            }
            Tok::CloseTag => {
                self.bump();
                self.stmt(StmtKind::Nop, start, doc)
            }
            Tok::OpenTagEcho => {
                self.bump();
                self.parse_expr_list_stmt(start, doc)
            }
            Tok::Punct("{") => {
                self.bump();
                let body = self.parse_until(true, &[]);
                self.expect_punct("}");
                self.stmt(StmtKind::Block(body), start, doc)
            }
            Tok::Punct(";") => {
                self.bump();
                self.stmt(StmtKind::Nop, start, doc)
            }
            Tok::Ident => self.parse_keyword_stmt(start, doc),
            _ => self.parse_expr_stmt(start, doc),
        }
    }

    fn parse_expr_stmt(&mut self, start: usize, doc: Option<Span>) -> Stmt {
        let e = self.parse_expr();
        self.expect_semi();
        self.stmt(StmtKind::Expr(e), start, doc)
    }

    fn parse_keyword_stmt(&mut self, start: usize, doc: Option<Span>) -> Stmt {
        let word = self.text().to_ascii_lowercase();
        let mut st = self.parse_keyword_stmt_inner(start, doc);
        if let StmtKind::Control { kw, .. } = &mut st.kind {
            *kw = match word.as_str() {
                "if" => "if",
                "while" => "while",
                "do" => "do",
                "for" => "for",
                "foreach" => "foreach",
                "switch" => "switch",
                _ => "declare",
            };
        }
        st
    }

    fn parse_keyword_stmt_inner(&mut self, start: usize, doc: Option<Span>) -> Stmt {
        let word = self.text().to_ascii_lowercase();
        match word.as_str() {
            "namespace" => self.parse_namespace(start, doc),
            "use" => self.parse_use_stmt(start, doc),
            "function" => self.parse_function_stmt(start, doc),
            "class" if !matches!(self.peek_at(1).kind, Tok::Ident) => self.parse_expr_stmt(start, doc),
            "abstract" | "final" | "class" | "interface" | "trait" => self.parse_class_stmt(start, doc),
            "readonly" if self.peek_at(1).kind == Tok::Ident => self.parse_class_stmt(start, doc),
            "enum"
                if self.peek_at(1).kind == Tok::Ident
                    && (self.peek_punct(2, "{") || self.peek_punct(2, ":") || self.peek_kw(2, "implements")) =>
            {
                self.parse_class_stmt(start, doc)
            }
            "const" => {
                self.bump();
                let items = self.parse_const_items(doc);
                self.expect_semi();
                self.stmt(StmtKind::Const(items), start, doc)
            }
            "if" => self.parse_if(start, doc),
            "while" => {
                self.bump();
                let header = self.parse_paren_exprs();
                let body = self.parse_body(&["endwhile"]);
                self.control(header, vec![body], start, doc)
            }
            "do" => self.parse_do(start, doc),
            "for" => {
                self.bump();
                let header = self.parse_for_header();
                let body = self.parse_body(&["endfor"]);
                self.control(header, vec![body], start, doc)
            }
            "foreach" => {
                self.bump();
                let header = self.parse_foreach_header();
                let body = self.parse_body(&["endforeach"]);
                self.control(header, vec![body], start, doc)
            }
            "switch" => self.parse_switch(start, doc),
            "try" => self.parse_try(start, doc),
            "declare" => self.parse_declare(start, doc),
            "return" | "break" | "continue" => {
                self.bump();
                let v = if self.at_stmt_end() { Vec::new() } else { vec![self.parse_expr()] };
                self.expect_semi();
                self.stmt(StmtKind::Exprs(v), start, doc)
            }
            "echo" | "global" => {
                self.bump();
                self.parse_expr_list_stmt(start, doc)
            }
            "static" if self.peek_at(1).kind == Tok::Variable => self.parse_static_vars(start, doc),
            "goto" => {
                self.bump();
                self.bump();
                self.expect_semi();
                self.stmt(StmtKind::Nop, start, doc)
            }
            "else" | "elseif" | "endif" | "endwhile" | "endfor" | "endforeach" | "endswitch" | "enddeclare"
            | "case" | "default" => {
                // stray block terminator
                self.error_here("unexpected keyword");
                self.bump();
                self.stmt(StmtKind::Nop, start, doc)
            }
            // `label:` (but not `parent::x()`)
            w if self.peek_punct(1, ":") && !matches!(w, "parent" | "self" | "static") && !self.peek_punct(2, ":") => {
                self.bump();
                self.bump();
                self.stmt(StmtKind::Nop, start, doc)
            }
            _ => self.parse_expr_stmt(start, doc),
        }
    }

    /// `function name() {}` is a declaration, `function () {}` a closure expression.
    fn parse_function_stmt(&mut self, start: usize, doc: Option<Span>) -> Stmt {
        // declaration iff followed by `name (` or `& name (`
        let n1 = self.peek_at(1);
        let is_decl =
            n1.kind == Tok::Ident || (matches!(n1.kind, Tok::Punct("&")) && self.peek_at(2).kind == Tok::Ident);
        if !is_decl {
            return self.parse_expr_stmt(start, doc);
        }
        let f = self.parse_function(doc, start);
        self.stmt(StmtKind::Function(Box::new(f)), start, doc)
    }

    /// `static $a = 1, $b;`
    fn parse_static_vars(&mut self, start: usize, doc: Option<Span>) -> Stmt {
        self.bump();
        let mut v = Vec::new();
        loop {
            v.push(self.parse_expr_bp(4));
            if !self.eat_punct(",") {
                break;
            }
        }
        self.expect_semi();
        self.stmt(StmtKind::Exprs(v), start, doc)
    }

    /// A loop / `declare`; `kw` is filled in by `parse_keyword_stmt`.
    fn control(&self, header: Vec<Expr>, blocks: Vec<Vec<Stmt>>, start: usize, doc: Option<Span>) -> Stmt {
        self.stmt(StmtKind::Control { kw: "", header, blocks }, start, doc)
    }

    fn parse_class_stmt(&mut self, start: usize, doc: Option<Span>) -> Stmt {
        let c = self.parse_class_decl(doc, start);
        self.stmt(StmtKind::Class(Box::new(c)), start, doc)
    }

    /// `expr, expr, ...;` (after `echo`, `global` or `<?=`)
    fn parse_expr_list_stmt(&mut self, start: usize, doc: Option<Span>) -> Stmt {
        let mut v = vec![self.parse_expr()];
        while self.eat_punct(",") {
            v.push(self.parse_expr());
        }
        self.expect_semi();
        self.stmt(StmtKind::Exprs(v), start, doc)
    }

    fn parse_do(&mut self, start: usize, doc: Option<Span>) -> Stmt {
        self.bump();
        let body = self.parse_body(&[]);
        let mut header = Vec::new();
        if self.eat_kw("while") {
            header = self.parse_paren_exprs();
        } else {
            self.error_here("expected 'while'");
        }
        self.expect_semi();
        self.control(header, vec![body], start, doc)
    }

    /// `(init; cond; step)`: every expression of the three parts.
    fn parse_for_header(&mut self) -> Vec<Expr> {
        let mut header = Vec::new();
        if self.expect_punct("(") {
            loop {
                if self.at_punct(")") || self.at_eof() {
                    break;
                }
                if self.at_punct(";") {
                    self.bump();
                    continue;
                }
                let before = self.pos;
                header.push(self.parse_expr());
                self.eat_punct(",");
                if self.pos == before {
                    self.bump();
                }
            }
            self.expect_punct(")");
        }
        header
    }

    /// `($subject as $value)` / `($subject as $key => $value)` -> `[subject, key?, value]`.
    fn parse_foreach_header(&mut self) -> Vec<Expr> {
        let mut header = Vec::new();
        if self.expect_punct("(") {
            header.push(self.parse_expr());
            if self.eat_kw("as") {
                self.eat_punct("&");
                let first = self.parse_expr();
                header.push(first);
                if self.eat_punct("=>") {
                    self.eat_punct("&");
                    header.push(self.parse_expr());
                }
            } else {
                self.error_here("expected 'as'");
            }
            self.expect_punct(")");
        }
        header
    }

    /// `declare(strict_types=1);` or `declare(ticks=1) { ... }`
    fn parse_declare(&mut self, start: usize, doc: Option<Span>) -> Stmt {
        self.bump();
        let mut header = Vec::new();
        if self.expect_punct("(") {
            while !self.at_punct(")") && !self.at_eof() && !self.at_stmt_end() {
                let before = self.pos;
                self.ident_here();
                if self.eat_punct("=") {
                    header.push(self.parse_expr());
                }
                self.eat_punct(",");
                if self.pos == before {
                    self.bump();
                }
            }
            self.expect_punct(")");
        }
        if self.at_stmt_end() {
            self.expect_semi();
            self.control(header, vec![], start, doc)
        } else {
            let body = self.parse_body(&["enddeclare"]);
            self.control(header, vec![body], start, doc)
        }
    }

    /// Parse statements until `}` (if `brace`), EOF, or one of the terminator keywords.
    fn parse_until(&mut self, brace: bool, kws: &[&str]) -> Vec<Stmt> {
        let mut out = Vec::new();
        loop {
            if self.at_eof() {
                break;
            }
            if brace && self.at_punct("}") {
                break;
            }
            if !kws.is_empty() && self.kind() == Tok::Ident {
                let t = self.text();
                if kws.iter().any(|k| t.eq_ignore_ascii_case(k)) {
                    break;
                }
            }
            let before = self.pos;
            out.push(self.parse_stmt());
            if self.pos == before {
                self.error_here("unexpected token");
                self.bump();
            }
        }
        out
    }

    /// Body of a control statement: `{ ... }`, a single statement, or alt syntax `: ... endX;`.
    fn parse_body(&mut self, alt_end: &[&str]) -> Vec<Stmt> {
        if self.at_punct("{") {
            self.bump();
            let b = self.parse_until(true, &[]);
            self.expect_punct("}");
            b
        } else if !alt_end.is_empty() && self.at_punct(":") {
            self.bump();
            let b = self.parse_until(false, alt_end);
            if self.eat_kw(alt_end[0]) {
                self.expect_semi();
            }
            b
        } else {
            vec![self.parse_stmt()]
        }
    }

    fn parse_paren_exprs(&mut self) -> Vec<Expr> {
        let mut v = Vec::new();
        if self.expect_punct("(") {
            if !self.at_punct(")") {
                v.push(self.parse_expr());
            }
            self.expect_punct(")");
        }
        v
    }

    fn parse_if(&mut self, start: usize, doc: Option<Span>) -> Stmt {
        self.bump();
        let mut header = self.parse_paren_exprs();
        let mut blocks = Vec::new();
        if self.at_punct(":") {
            self.bump();
            blocks.push(self.parse_until(false, &["elseif", "else", "endif"]));
            loop {
                if self.at_kw("elseif") {
                    self.bump();
                    header.extend(self.parse_paren_exprs());
                    self.eat_punct(":");
                    blocks.push(self.parse_until(false, &["elseif", "else", "endif"]));
                } else if self.at_kw("else") && self.peek_kw(1, "if") {
                    self.bump();
                    self.bump();
                    header.extend(self.parse_paren_exprs());
                    self.eat_punct(":");
                    blocks.push(self.parse_until(false, &["elseif", "else", "endif"]));
                } else if self.at_kw("else") {
                    self.bump();
                    self.eat_punct(":");
                    blocks.push(self.parse_until(false, &["endif"]));
                } else {
                    break;
                }
            }
            if self.eat_kw("endif") {
                self.expect_semi();
            }
        } else {
            blocks.push(self.parse_body(&[]));
            loop {
                if self.at_kw("elseif") {
                    self.bump();
                    header.extend(self.parse_paren_exprs());
                    blocks.push(self.parse_body(&[]));
                } else if self.at_kw("else") {
                    self.bump();
                    blocks.push(self.parse_body(&[]));
                    break;
                } else {
                    break;
                }
            }
        }
        self.stmt(StmtKind::Control { kw: "", header, blocks }, start, doc)
    }

    fn parse_switch(&mut self, start: usize, doc: Option<Span>) -> Stmt {
        self.bump();
        let mut header = self.parse_paren_exprs();
        let mut blocks = Vec::new();
        let alt = self.at_punct(":");
        if alt || self.at_punct("{") {
            self.bump();
            loop {
                if self.at_eof() || self.at_punct("}") || self.at_kw("endswitch") {
                    break;
                }
                if self.eat_kw("case") {
                    header.push(self.parse_expr());
                    if !self.eat_punct(":") {
                        self.eat_punct(";");
                    }
                } else if self.eat_kw("default") {
                    if !self.eat_punct(":") {
                        self.eat_punct(";");
                    }
                } else {
                    let before = self.pos;
                    blocks.push(vec![self.parse_stmt()]);
                    if self.pos == before {
                        self.bump();
                    }
                    continue;
                }
                blocks.push(self.parse_until(true, &["case", "default", "endswitch"]));
            }
            if alt {
                if self.eat_kw("endswitch") {
                    self.expect_semi();
                }
            } else {
                self.expect_punct("}");
            }
        } else {
            self.error_here("expected '{'");
        }
        self.stmt(StmtKind::Control { kw: "", header, blocks }, start, doc)
    }

    fn parse_try(&mut self, start: usize, doc: Option<Span>) -> Stmt {
        self.bump();
        let body = self.parse_braced_block();
        let mut catches = Vec::new();
        let mut finally = None;
        loop {
            if self.at_kw("catch") {
                self.bump();
                let mut types = Vec::new();
                let mut var = None;
                if self.expect_punct("(") {
                    while let Some(n) = self.name_here() {
                        types.push(n);
                        if !self.eat_punct("|") {
                            break;
                        }
                    }
                    if self.kind() == Tok::Variable {
                        let t = self.bump();
                        var = Some(Ident { text: self.text_of(t)[1..].to_string(), span: t.span });
                    }
                    self.expect_punct(")");
                }
                let body = self.parse_braced_block();
                catches.push(Catch { types, var, body });
            } else if self.at_kw("finally") {
                self.bump();
                finally = Some(self.parse_braced_block());
            } else {
                break;
            }
        }
        self.stmt(StmtKind::Try { body, catches, finally }, start, doc)
    }

    pub(super) fn parse_braced_block(&mut self) -> Vec<Stmt> {
        if self.expect_punct("{") {
            let b = self.parse_until(true, &[]);
            self.expect_punct("}");
            b
        } else {
            Vec::new()
        }
    }

    fn parse_namespace(&mut self, start: usize, doc: Option<Span>) -> Stmt {
        self.bump();
        let name = self.name_here();
        if self.at_punct("{") {
            self.bump();
            let body = self.parse_until(true, &[]);
            self.expect_punct("}");
            self.stmt(StmtKind::Namespace { name, body: Some(body) }, start, doc)
        } else {
            self.expect_semi();
            self.stmt(StmtKind::Namespace { name, body: None }, start, doc)
        }
    }

    fn parse_use_stmt(&mut self, start: usize, doc: Option<Span>) -> Stmt {
        self.bump();
        let mut kind = UseKind::Class;
        if self.at_kw("function") {
            self.bump();
            kind = UseKind::Function;
        } else if self.at_kw("const") {
            self.bump();
            kind = UseKind::Const;
        }
        let mut items = Vec::new();
        loop {
            let Some(n) = self.name_here() else {
                self.error_here("expected name");
                break;
            };
            let base = n.text.trim_start_matches('\\').to_string();
            if self.at_punct("\\") && self.peek_punct(1, "{") {
                self.bump();
                self.bump();
                loop {
                    if self.at_punct("}") || self.at_eof() {
                        break;
                    }
                    let mut k = kind;
                    if self.at_kw("function") && self.peek_at(1).kind != Tok::Punct(",") {
                        self.bump();
                        k = UseKind::Function;
                    } else if self.at_kw("const") && self.peek_at(1).kind != Tok::Punct(",") {
                        self.bump();
                        k = UseKind::Const;
                    }
                    let Some(m) = self.name_here() else {
                        self.bump();
                        continue;
                    };
                    let alias = self.parse_use_alias();
                    items.push(UseItem {
                        kind: k,
                        name: format!("{}\\{}", base.trim_end_matches('\\'), m.text),
                        alias,
                        span: m.span,
                    });
                    if !self.eat_punct(",") {
                        break;
                    }
                }
                self.expect_punct("}");
            } else {
                let alias = self.parse_use_alias();
                items.push(UseItem { kind, name: base, alias, span: n.span });
            }
            if !self.eat_punct(",") {
                break;
            }
        }
        self.expect_semi();
        self.stmt(StmtKind::Use(items), start, doc)
    }

    fn parse_use_alias(&mut self) -> Option<String> {
        if self.eat_kw("as") {
            self.ident_here().map(|i| i.text)
        } else {
            None
        }
    }

    pub(super) fn parse_const_items(&mut self, doc: Option<Span>) -> Vec<ConstItem> {
        let mut items = Vec::new();
        loop {
            // typed class constants: `const string FOO = ...`
            if self.kind() == Tok::Ident && matches!(self.peek_at(1).kind, Tok::Ident | Tok::Name) {
                self.bump();
            }
            let Some(name) = self.ident_here() else {
                self.error_here("expected constant name");
                break;
            };
            let value = if self.eat_punct("=") { self.parse_expr() } else { self.err_expr() };
            items.push(ConstItem { name, value, doc });
            if !self.eat_punct(",") {
                break;
            }
        }
        items
    }
}
