//! Trailing commas (`COMMA_AFTER_LAST_*`) and short array syntax
//! (`FORCE_SHORT_DECLARATION_ARRAY_STYLE`).
//!
//! - `array(…)` becomes `[…]` (before the layout passes);
//! - a comma goes after the last item of a list whose closing bracket is on a line of its own
//!   (`WHEN_MULTILINE`, or always with `ALWAYS`): arrays, arguments (PHP 7.3+), parameters and
//!   closure `use` lists (PHP 8.0+), `match` arms. Runs after wrapping, since only then it is
//!   known which lists span lines.
//!
//! Both are no-ops for PHP; the final check ignores commas right before a closing bracket and
//! reads `array(` as `[`.

use crate::options::FormatOptions;
use crate::space::{bracket_contexts, Ctx, Edit};
use php_syntax::lexer::{Tok, Token};

/// Significant token indices (no comments / open tags).
fn sig(toks: &[Token]) -> Vec<usize> {
    (0..toks.len()).filter(|&i| !matches!(toks[i].kind, Tok::Comment | Tok::OpenTag | Tok::Eof)).collect()
}

/// `array(` → `[`, its `)` → `]`. None when nothing changes.
pub(crate) fn short_arrays(src: &str, toks: &[Token], o: &FormatOptions) -> Option<String> {
    if !o.FORCE_SHORT_DECLARATION_ARRAY_STYLE {
        return None;
    }
    let ctxs = bracket_contexts(src, toks, o);
    let mut edits: Vec<Edit> = Vec::new();
    let mut stack: Vec<(usize, bool)> = Vec::new();
    for &(i, ctx) in &ctxs {
        match toks[i].span.text(src) {
            "(" | "[" | "#[" | "{" => {
                let convert = ctx == Ctx::ArrayCall && {
                    // the `array` keyword right before, with nothing but whitespace between
                    let prev = (0..i).rev().find(|&j| toks[j].kind != Tok::Comment);
                    prev.is_some_and(|p| p + 1 == i && toks[p].span.text(src).eq_ignore_ascii_case("array"))
                };
                if convert {
                    let p = i - 1;
                    edits.push(Edit {
                        start: toks[p].span.start as usize,
                        end: toks[i].span.end as usize,
                        text: "[".into(),
                    });
                }
                stack.push((i, convert));
            }
            _ => {
                if let Some((_, true)) = stack.pop() {
                    let sp = toks[i].span;
                    edits.push(Edit { start: sp.start as usize, end: sp.end as usize, text: "]".into() });
                }
            }
        }
    }
    if edits.is_empty() {
        return None;
    }
    edits.sort_by_key(|e| e.start);
    let mut out = String::with_capacity(src.len());
    let mut at = 0;
    for e in &edits {
        out.push_str(&src[at..e.start]);
        out.push_str(&e.text);
        at = e.end;
    }
    out.push_str(&src[at..]);
    Some(out)
}

/// Should a list of this kind get a trailing comma, given whether it spans lines?
fn wanted(enabled: bool, style: &str, multiline: bool) -> bool {
    enabled && (multiline || style.eq_ignore_ascii_case("ALWAYS"))
}

/// Commas to insert after the last item of lists (formatted `src`), for PHP `php`.
pub(crate) fn trailing_commas(src: &str, toks: &[Token], o: &FormatOptions, php: (u8, u8)) -> Vec<Edit> {
    let any = o.COMMA_AFTER_LAST_ARRAY_ELEMENT
        || o.COMMA_AFTER_LAST_ARGUMENT
        || o.COMMA_AFTER_LAST_PARAMETER
        || o.COMMA_AFTER_LAST_CLOSURE_USE_VAR
        || o.COMMA_AFTER_LAST_MATCH_ARM;
    // templates: no tokens are added
    let template = toks.iter().any(|t| {
        t.kind == Tok::InlineHtml && !t.span.text(src).trim().is_empty() && !t.span.text(src).starts_with("#!")
    });
    if !any || template {
        return Vec::new();
    }
    let s = sig(toks);
    // position in `s` of each token index
    let mut pos = vec![usize::MAX; toks.len()];
    for (k, &i) in s.iter().enumerate() {
        pos[i] = k;
    }
    let text = |i: usize| toks[i].span.text(src);
    let first_on_line =
        |i: usize| i == 0 || src[toks[i - 1].span.end as usize..toks[i].span.start as usize].contains('\n');
    let ctxs = bracket_contexts(src, toks, o);
    let mut stack: Vec<(usize, Ctx)> = Vec::new();
    let mut edits = Vec::new();
    for &(i, ctx) in &ctxs {
        match text(i) {
            "(" | "[" | "#[" | "{" => stack.push((i, ctx)),
            _ => {
                let Some((open, ctx)) = stack.pop() else { continue };
                let k = pos[i];
                if k == usize::MAX || k == 0 {
                    continue;
                }
                let last = s[k - 1];
                // `f(...)` is a first-class callable, not a call with an argument
                if last == open || text(last) == "," || text(last) == "..." && pos[last] > 0 && s[pos[last] - 1] == open
                {
                    continue;
                }
                let multiline = first_on_line(i);
                let before_open = pos[open].checked_sub(1).map(|p| s[p]);
                let construct = before_open.is_some_and(|p| {
                    toks[p].kind == Tok::Ident
                        && ["isset", "empty", "unset", "exit", "die", "eval", "list"]
                            .iter()
                            .any(|w| text(p).eq_ignore_ascii_case(w))
                });
                // destructuring (`list(…) = `, `[…] = `, `as […]`): a comma there would add an element
                let after_close = s.get(k + 1).map(|&n| text(n));
                let destructuring = before_open
                    .is_some_and(|p| text(p).eq_ignore_ascii_case("list") || text(p).eq_ignore_ascii_case("as"))
                    || after_close == Some("=");
                let add = match ctx {
                    Ctx::ArrayLit | Ctx::ArrayCall if destructuring => false,
                    Ctx::ArrayLit | Ctx::ArrayCall => {
                        wanted(o.COMMA_AFTER_LAST_ARRAY_ELEMENT, "WHEN_MULTILINE", multiline)
                    }
                    Ctx::Call | Ctx::Attr if text(i) == ")" && !construct => {
                        php >= (7, 3)
                            && wanted(o.COMMA_AFTER_LAST_ARGUMENT, &o.COMMA_AFTER_LAST_ARGUMENT_STYLE, multiline)
                    }
                    Ctx::Decl | Ctx::ClosureDecl => {
                        php >= (8, 0)
                            && wanted(o.COMMA_AFTER_LAST_PARAMETER, &o.COMMA_AFTER_LAST_PARAMETER_STYLE, multiline)
                    }
                    Ctx::ClosureUse => {
                        php >= (8, 0)
                            && wanted(
                                o.COMMA_AFTER_LAST_CLOSURE_USE_VAR,
                                &o.COMMA_AFTER_LAST_CLOSURE_USE_VAR_STYLE,
                                multiline,
                            )
                    }
                    // `match (…) { … }`
                    Ctx::Block if text(i) == "}" => {
                        let is_match = before_open.is_some_and(|p| text(p) == ")") && {
                            // the `(` of that `)` follows `match`
                            let mut depth = 0i32;
                            let mut j = pos[before_open.unwrap_or(0)];
                            let mut m = false;
                            loop {
                                match text(s[j]) {
                                    ")" => depth += 1,
                                    "(" => {
                                        depth -= 1;
                                        if depth == 0 {
                                            // the `match` keyword, not a method named so
                                            // (`function match(`, `$x->match(`)
                                            m = j > 0
                                                && text(s[j - 1]).eq_ignore_ascii_case("match")
                                                && !(j > 1
                                                    && ["function", "fn", "->", "?->", "::", "const"]
                                                        .iter()
                                                        .any(|w| text(s[j - 2]).eq_ignore_ascii_case(w)));
                                            break;
                                        }
                                    }
                                    _ => {}
                                }
                                if j == 0 {
                                    break;
                                }
                                j -= 1;
                            }
                            m
                        };
                        is_match && multiline && o.COMMA_AFTER_LAST_MATCH_ARM
                    }
                    _ => false,
                };
                // a heredoc closing label needs PHP 7.3 for anything after it on its line
                let heredoc = text(last).starts_with("<<<");
                if add && !(heredoc && php < (7, 3)) {
                    let at = toks[last].span.end as usize;
                    edits.push(Edit { start: at, end: at, text: ",".into() });
                }
            }
        }
    }
    edits.sort_by_key(|e| e.start);
    edits
}
