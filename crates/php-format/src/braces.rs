//! The one pass that changes more than whitespace: braces around single-statement bodies
//! (`IF_BRACE_FORCE`, `FOR_BRACE_FORCE`, `WHILE_BRACE_FORCE`, `DOWHILE_BRACE_FORCE`) and
//! `else if` ↔ `elseif` (`ELSE_IF_STYLE`).
//!
//! `if ($a) foo();` becomes `if ($a) { foo(); }` (the later passes put it on lines). The end of a
//! body is found by walking the statement: `if` with its `elseif` / `else` chain (so a dangling
//! `else` stays with its `if`), loops, `do … while`, `switch`, `try`, blocks, declarations, and
//! otherwise up to the `;`. When anything is unclear (alternative syntax, `?>` ending a
//! statement) that body is left alone.
//!
//! [`verify`] checks the result: without the inserted braces, and with `else if` and `elseif`
//! counted as the same, the tokens must be exactly the original ones.

use crate::options::FormatOptions;
use php_syntax::lexer::{lex_with_trivia, Tok, Token};

/// PhpStorm's `*_BRACE_FORCE` values.
const FORCE_IF_MULTILINE: usize = 1;
const FORCE_ALWAYS: usize = 3;

/// The rewritten text and where the inserted braces are in it (byte offsets).
pub(crate) struct Rewrite {
    pub text: String,
    pub inserted: Vec<usize>,
}

struct Walker<'a> {
    src: &'a str,
    toks: &'a [Token],
    /// Significant tokens (no comments / open tags) as indices into `toks`.
    sig: Vec<usize>,
}

impl Walker<'_> {
    fn text(&self, k: usize) -> &str {
        self.toks[self.sig[k]].span.text(self.src)
    }

    fn is_kw(&self, k: usize, kw: &str) -> bool {
        k < self.sig.len() && self.toks[self.sig[k]].kind == Tok::Ident && self.text(k).eq_ignore_ascii_case(kw)
    }

    fn is(&self, k: usize, t: &str) -> bool {
        k < self.sig.len() && self.text(k) == t
    }

    /// Index of the bracket matching the opener at `k`.
    fn matching(&self, k: usize) -> Option<usize> {
        let mut depth = 0i32;
        for j in k..self.sig.len() {
            match self.text(j) {
                "(" | "[" | "{" | "#[" => depth += 1,
                ")" | "]" | "}" => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(j);
                    }
                    if depth < 0 {
                        return None;
                    }
                }
                _ => {}
            }
            if matches!(self.toks[self.sig[j]].kind, Tok::CloseTag | Tok::InlineHtml | Tok::Eof) {
                return None;
            }
        }
        None
    }

    /// `(…)` at `k` → index after it.
    fn header(&self, k: usize) -> Option<usize> {
        if !self.is(k, "(") {
            return None;
        }
        Some(self.matching(k)? + 1)
    }

    /// Last token of the statement starting at `k`.
    fn stmt_end(&self, k: usize) -> Option<usize> {
        if k >= self.sig.len() {
            return None;
        }
        let kind = self.toks[self.sig[k]].kind;
        if matches!(kind, Tok::CloseTag | Tok::InlineHtml | Tok::Eof | Tok::OpenTagEcho) {
            return None;
        }
        if self.is(k, "{") {
            return self.matching(k);
        }
        if self.is(k, ";") {
            return Some(k);
        }
        if kind == Tok::Ident && !self.is(k + 1, "::") {
            let lower = self.text(k).to_ascii_lowercase();
            match lower.as_str() {
                "if" => {
                    let body = self.header(k + 1)?;
                    if self.is(body, ":") {
                        return None;
                    }
                    let mut end = self.stmt_end(body)?;
                    loop {
                        if self.is_kw(end + 1, "elseif") {
                            let b = self.header(end + 2)?;
                            if self.is(b, ":") {
                                return None;
                            }
                            end = self.stmt_end(b)?;
                        } else if self.is_kw(end + 1, "else") {
                            if self.is(end + 2, ":") {
                                return None;
                            }
                            return self.stmt_end(end + 2);
                        } else {
                            return Some(end);
                        }
                    }
                }
                "while" | "for" | "foreach" | "declare" => {
                    let body = self.header(k + 1)?;
                    if self.is(body, ":") {
                        return None;
                    }
                    self.stmt_end(body)
                }
                "switch" => {
                    let body = self.header(k + 1)?;
                    if !self.is(body, "{") {
                        return None;
                    }
                    self.matching(body)
                }
                "do" => {
                    let end = self.stmt_end(k + 1)?;
                    if !self.is_kw(end + 1, "while") {
                        return None;
                    }
                    let after = self.header(end + 2)?;
                    self.is(after, ";").then_some(after)
                }
                "try" => {
                    let mut end = self.matching(k + 1).filter(|_| self.is(k + 1, "{"))?;
                    loop {
                        if self.is_kw(end + 1, "catch") {
                            let b = self.header(end + 2)?;
                            end = self.matching(b).filter(|_| self.is(b, "{"))?;
                        } else if self.is_kw(end + 1, "finally") {
                            end = self.matching(end + 2).filter(|_| self.is(end + 2, "{"))?;
                        } else {
                            return Some(end);
                        }
                    }
                }
                // declarations end with their body
                "function" | "class" | "interface" | "trait" | "enum" | "abstract" | "final" | "readonly" => {
                    let mut j = k;
                    while j < self.sig.len() {
                        match self.text(j) {
                            "{" => return self.matching(j),
                            ";" => return Some(j),
                            "(" | "[" => j = self.matching(j)?,
                            _ => {}
                        }
                        j += 1;
                    }
                    None
                }
                _ => self.simple_end(k),
            }
        } else {
            self.simple_end(k)
        }
    }

    /// A simple statement: up to the `;` outside brackets.
    fn simple_end(&self, k: usize) -> Option<usize> {
        let mut j = k;
        while j < self.sig.len() {
            match self.toks[self.sig[j]].kind {
                Tok::CloseTag | Tok::InlineHtml | Tok::Eof => return None,
                _ => {}
            }
            match self.text(j) {
                ";" => return Some(j),
                "(" | "[" | "{" | "#[" => j = self.matching(j)?,
                ")" | "]" | "}" => return None,
                _ => {}
            }
            j += 1;
        }
        None
    }
}

/// Insert braces / rewrite `else if`; None when nothing changes.
pub(crate) fn rewrite(src: &str, toks: &[Token], o: &FormatOptions) -> Option<Rewrite> {
    // templates are left alone
    if toks.iter().any(|t| {
        t.kind == Tok::InlineHtml
            && !t.span.text(src).trim().is_empty()
            && !(t.span.start == 0 && t.span.text(src).starts_with("#!"))
    }) {
        return None;
    }
    let sig: Vec<usize> =
        (0..toks.len()).filter(|&i| !matches!(toks[i].kind, Tok::Comment | Tok::OpenTag | Tok::Eof)).collect();
    let w = Walker { src, toks, sig };
    let line_of = |off: usize| src[..off].matches('\n').count();
    // (byte offset, order, text): text inserted at an offset; at the same offset, lower order first
    let mut inserts: Vec<(usize, i64, &'static str)> = Vec::new();
    // byte ranges replaced: (start, end, text)
    let mut replaces: Vec<(usize, usize, &'static str)> = Vec::new();
    let mut n = 0i64;
    let mut do_whiles = std::collections::HashSet::new();
    let combine = o.ELSE_IF_STYLE.eq_ignore_ascii_case("COMBINE");
    let separate = o.ELSE_IF_STYLE.eq_ignore_ascii_case("SEPARATE");
    for k in 0..w.sig.len() {
        if w.toks[w.sig[k]].kind != Tok::Ident {
            continue;
        }
        // a keyword, not a name (`$x->if`, `A::for`, `function while`)
        if k > 0
            && (matches!(w.text(k - 1), "->" | "?->" | "::") || w.is_kw(k - 1, "function") || w.is_kw(k - 1, "const"))
        {
            continue;
        }
        let lower = w.text(k).to_ascii_lowercase();
        let (force, body) = match lower.as_str() {
            "if" | "elseif" => (o.IF_BRACE_FORCE, w.header(k + 1)),
            "else" => {
                // `else if`: a chain, the `if` gets its own braces
                if w.is_kw(k + 1, "if") {
                    let gap_has_comment = w.sig[k + 1] != w.sig[k] + 1;
                    if combine && !gap_has_comment {
                        let (a, b) = (w.toks[w.sig[k]].span, w.toks[w.sig[k + 1]].span);
                        replaces.push((a.start as usize, b.end as usize, "elseif"));
                    }
                    continue;
                }
                (o.IF_BRACE_FORCE, Some(k + 1))
            }
            "for" | "foreach" => (o.FOR_BRACE_FORCE, w.header(k + 1)),
            // the `while` of `do … while` has no body
            "while" if do_whiles.contains(&k) => continue,
            "while" => (o.WHILE_BRACE_FORCE, w.header(k + 1)),
            "do" => {
                if let Some(end) = w.stmt_end(k + 1) {
                    if w.is_kw(end + 1, "while") {
                        do_whiles.insert(end + 1);
                    }
                }
                (o.DOWHILE_BRACE_FORCE, Some(k + 1))
            }
            _ => {
                if separate && lower == "elseif" {
                    let sp = w.toks[w.sig[k]].span;
                    replaces.push((sp.start as usize, sp.end as usize, "else if"));
                }
                continue;
            }
        };
        if separate && lower == "elseif" {
            let sp = w.toks[w.sig[k]].span;
            replaces.push((sp.start as usize, sp.end as usize, "else if"));
        }
        if force == 0 {
            continue;
        }
        let Some(b) = body else { continue };
        if b >= w.sig.len() || w.is(b, "{") || w.is(b, ":") || w.is(b, ";") {
            continue;
        }
        let Some(end) = w.stmt_end(b) else { continue };
        let multiline =
            line_of(w.toks[w.sig[k]].span.start as usize) != line_of(w.toks[w.sig[end]].span.start as usize);
        if !(force == FORCE_ALWAYS || force == FORCE_IF_MULTILINE && multiline) {
            continue;
        }
        n += 1;
        // `{` right after the header, `}` right after the body (inner bodies close first)
        let open_at = w.toks[w.sig[b - 1]].span.end as usize;
        let close_at = w.toks[w.sig[end]].span.end as usize;
        inserts.push((open_at, n, " {"));
        inserts.push((close_at, -n, " }"));
    }
    if inserts.is_empty() && replaces.is_empty() {
        return None;
    }
    inserts.sort_by_key(|&(at, order, _)| (at, order));
    replaces.sort_by_key(|r| r.0);
    let mut text = String::with_capacity(src.len() + inserts.len() * 2);
    let mut inserted = Vec::new();
    let mut at = 0;
    let (mut ii, mut ri) = (0, 0);
    loop {
        let next_ins = inserts.get(ii).map(|x| x.0);
        let next_rep = replaces.get(ri).map(|x| x.0);
        let insert_first = match (next_ins, next_rep) {
            (None, None) => break,
            (Some(p), Some(r)) => p <= r,
            (Some(_), None) => true,
            (None, Some(_)) => false,
        };
        match (insert_first, next_ins) {
            (true, Some(p)) => {
                text.push_str(&src[at..p]);
                at = p;
                let s = inserts[ii].2;
                inserted.push(text.len() + s.len() - 1);
                text.push_str(s);
                ii += 1;
            }
            _ => {
                let (s, e, t) = replaces[ri];
                text.push_str(&src[at..s]);
                text.push_str(t);
                at = e;
                ri += 1;
            }
        }
    }
    text.push_str(&src[at..]);
    Some(Rewrite { text, inserted })
}

/// Tokens of `src` as (kind, text), with `else` `if` read as `elseif`.
fn canonical(src: &str, skip: &[usize]) -> Vec<(Tok, String)> {
    let toks = lex_with_trivia(src);
    let mut out: Vec<(Tok, String)> = Vec::with_capacity(toks.len());
    let mut skip = skip.iter().peekable();
    for t in &toks {
        let start = t.span.start as usize;
        while skip.peek().is_some_and(|&&s| s < start) {
            skip.next();
        }
        if skip.peek().is_some_and(|&&s| s == start) {
            continue;
        }
        let text = t.span.text(src).to_string();
        if t.kind == Tok::Ident && text.eq_ignore_ascii_case("if") {
            if let Some(last) = out.last_mut() {
                if last.0 == Tok::Ident && last.1.eq_ignore_ascii_case("else") {
                    last.1 = "elseif".into();
                    continue;
                }
            }
        }
        let text = if t.kind == Tok::Ident && text.eq_ignore_ascii_case("elseif") { "elseif".into() } else { text };
        out.push((t.kind, text));
    }
    out
}

/// Is `rw` exactly `src` plus braces at `rw.inserted` (and `else if` / `elseif` changes)?
pub(crate) fn verify(src: &str, rw: &Rewrite) -> bool {
    canonical(src, &[]) == canonical(&rw.text, &rw.inserted)
}
