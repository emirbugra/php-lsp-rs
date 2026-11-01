//! Variables inside double-quoted strings, heredocs and backticks.

use super::Parser;
use crate::ast::*;
use crate::lexer;
use crate::span::Span;

impl Parser<'_> {
    /// Expressions embedded in the template token at `span`: `$a`, `$a->b`, `$a[..]`, `{$expr}`, `${name}`.
    pub(super) fn parse_template(&mut self, span: Span) -> Vec<Expr> {
        let b = self.src.as_bytes();
        let (mut i, end) = self.template_content(span);
        let mut parts = Vec::new();
        while i < end {
            match b[i] {
                b'\\' => i += 2,
                b'{' if i + 1 < end && b[i + 1] == b'$' => {
                    let (expr, next) = self.braced_expr(i, end);
                    parts.push(expr);
                    i = next;
                }
                b'$' if i + 1 < end && b[i + 1] == b'{' => {
                    // ${name} / ${expr}
                    let mut j = i + 2;
                    while j < end && b[j] != b'}' {
                        j += 1;
                    }
                    let inner = &self.src[i + 2..j.min(end)];
                    if !inner.is_empty() && inner.bytes().all(lexer::is_ident_char) {
                        parts.push(Expr { kind: ExprKind::Var(inner.to_string()), span: Span::new(i + 2, j) });
                    }
                    i = j + 1;
                }
                b'$' if i + 1 < end && lexer::is_ident_start(b[i + 1]) => {
                    let (expr, next) = self.simple_interpolation(i, end);
                    parts.push(expr);
                    i = next;
                }
                _ => i += 1,
            }
        }
        parts
    }

    /// Byte range of the content: without the quotes, or without a heredoc's first and last line.
    fn template_content(&self, span: Span) -> (usize, usize) {
        let (src, b) = (self.src, self.src.as_bytes());
        let s = span.start as usize;
        let e = span.end as usize;
        if b[s] == b'<' {
            // heredoc: skip header line, drop closing label line
            let i = src[s..e].find('\n').map(|r| s + r + 1).unwrap_or(e);
            let end = src[i..e].rfind('\n').map(|r| i + r).unwrap_or(i);
            return (i, end);
        }
        let end = if e > s + 1 && (b[e - 1] == b'"' || b[e - 1] == b'`') { e - 1 } else { e };
        (s + 1, end)
    }

    /// `{$expr}` starting at `i`: the parsed expression and the position after the closing brace.
    fn braced_expr(&self, i: usize, end: usize) -> (Expr, usize) {
        let b = self.src.as_bytes();
        // find matching brace
        let mut depth = 0i32;
        let mut j = i;
        while j < end {
            match b[j] {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                b'\'' | b'"' => {
                    let q = b[j];
                    j += 1;
                    while j < end && b[j] != q {
                        if b[j] == b'\\' {
                            j += 1;
                        }
                        j += 1;
                    }
                }
                _ => {}
            }
            j += 1;
        }
        let toks = lexer::lex_php_range(self.src, i + 1, j.min(end));
        let mut sub = Parser::new(self.src, toks);
        sub.depth = self.depth;
        (sub.parse_expr(), j + 1)
    }

    /// `$name`, `$name->prop` or `$name[...]` starting at `i`, and the position after it.
    fn simple_interpolation(&self, i: usize, end: usize) -> (Expr, usize) {
        let (src, b) = (self.src, self.src.as_bytes());
        let mut j = i + 1;
        while j < end && lexer::is_ident_char(b[j]) {
            j += 1;
        }
        let var = Expr { kind: ExprKind::Var(src[i + 1..j].to_string()), span: Span::new(i, j) };
        if j + 2 < end && b[j] == b'-' && b[j + 1] == b'>' && lexer::is_ident_start(b[j + 2]) {
            let ns = j + 2;
            let mut k = ns;
            while k < end && lexer::is_ident_char(b[k]) {
                k += 1;
            }
            let name = Ident { text: src[ns..k].to_string(), span: Span::new(ns, k) };
            let prop = ExprKind::Prop { obj: Box::new(var), name: MemberName::Ident(name), nullsafe: false };
            return (Expr { kind: prop, span: Span::new(i, k) }, k);
        }
        if j < end && b[j] == b'[' {
            let mut k = j;
            while k < end && b[k] != b']' {
                k += 1;
            }
            return (var, (k + 1).min(end));
        }
        (var, j)
    }
}
