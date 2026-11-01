//! Column alignment on formatted text: `=>` in arrays (`ALIGN_KEY_VALUE_PAIRS`), `=` of
//! consecutive assignments (`ALIGN_ASSIGNMENTS`) and class constants (`ALIGN_CLASS_CONSTANTS`),
//! trailing comments of consecutive lines (`ALIGN_INLINE_COMMENTS`).
//!
//! Only the gap in front of the aligned token changes. The target column is worked out from the
//! end of what comes before (not from the gap as it is), so formatting again gives the same.

use crate::options::FormatOptions;
use crate::space::{bracket_contexts, Ctx, Edit};
use php_syntax::lexer::{Tok, Token};

struct Text<'a> {
    src: &'a str,
    toks: &'a [Token],
    o: &'a FormatOptions,
    line_starts: Vec<usize>,
}

impl Text<'_> {
    fn line_of(&self, off: usize) -> usize {
        self.line_starts.partition_point(|&s| s <= off) - 1
    }

    fn text(&self, i: usize) -> &str {
        self.toks[i].span.text(self.src)
    }

    /// Column where token `i` ends.
    fn end_col(&self, i: usize) -> usize {
        let end = self.toks[i].span.end as usize;
        self.o.width(&self.src[self.line_starts[self.line_of(end.saturating_sub(1))]..end])
    }

    fn first_on_line(&self, i: usize) -> bool {
        i == 0 || self.src[self.toks[i - 1].span.end as usize..self.toks[i].span.start as usize].contains('\n')
    }

    /// Previous token that is not a comment.
    fn prev_sig(&self, i: usize) -> Option<usize> {
        (0..i).rev().find(|&j| !matches!(self.toks[j].kind, Tok::Comment | Tok::OpenTag))
    }
}

/// Pad the gaps before `targets` (token indices) so that the targets end in the same column
/// (`=` and `.=` end together), one space after the furthest token before them.
fn align_group(t: &Text<'_>, targets: &[usize], edits: &mut Vec<Edit>) {
    align_targets(t, targets, true, edits)
}

/// Like [`align_group`]; `by_end` false lines up the starts (comments).
fn align_targets(t: &Text<'_>, targets: &[usize], by_end: bool, edits: &mut Vec<Edit>) {
    if targets.len() < 2 {
        return;
    }
    let w = |i: usize| if by_end { t.text(i).chars().count() } else { 0 };
    // single-character targets would also do with left alignment; operators end together
    let col = targets.iter().map(|&i| t.end_col(i - 1) + 1 + w(i)).max().unwrap_or(0);
    for &i in targets {
        let pad = col - w(i) - t.end_col(i - 1);
        let (start, end) = (t.toks[i - 1].span.end as usize, t.toks[i].span.start as usize);
        let want = " ".repeat(pad);
        if t.src[start..end] != want {
            edits.push(Edit { start, end, text: want });
        }
    }
}

const ASSIGN: &[&str] = &["=", "+=", "-=", "*=", "/=", ".=", "%=", "**=", "??=", "&=", "|=", "^=", "<<=", ">>="];

/// Alignment edits for formatted `src`: `=>` / `=` first (`comments` false), then trailing
/// comments on the result (`comments` true), whose columns depend on the first step.
pub(crate) fn align(src: &str, toks: &[Token], o: &FormatOptions, comments: bool) -> Vec<Edit> {
    let wanted = if comments {
        o.ALIGN_INLINE_COMMENTS
    } else {
        o.ALIGN_KEY_VALUE_PAIRS || o.ALIGN_ASSIGNMENTS || o.ALIGN_CLASS_CONSTANTS
    };
    let template = toks.iter().any(|t| {
        t.kind == Tok::InlineHtml && !t.span.text(src).trim().is_empty() && !t.span.text(src).starts_with("#!")
    });
    if !wanted || template {
        return Vec::new();
    }
    let mut line_starts = vec![0usize];
    line_starts.extend(src.match_indices('\n').map(|(i, _)| i + 1));
    let t = Text { src, toks, o, line_starts };
    let mut edits = Vec::new();

    // `=>` of the items of each array that start their line
    if o.ALIGN_KEY_VALUE_PAIRS && !comments {
        let ctxs = bracket_contexts(src, toks, o);
        // (opener, context, `=>` to align, last comma at this level)
        let mut stack: Vec<(usize, Ctx, Vec<usize>, Option<usize>)> = Vec::new();
        let mut ci = 0;
        for i in 0..toks.len() {
            if ci < ctxs.len() && ctxs[ci].0 == i {
                let ctx = ctxs[ci].1;
                ci += 1;
                if matches!(toks[i].kind, Tok::Punct("(" | "[" | "#[" | "{")) {
                    stack.push((i, ctx, Vec::new(), None));
                } else if let Some((_, ctx, arrows, _)) = stack.pop() {
                    if matches!(ctx, Ctx::ArrayLit | Ctx::ArrayCall) {
                        align_group(&t, &arrows, &mut edits);
                    }
                }
                continue;
            }
            if let Some(top) = stack.last_mut() {
                if toks[i].kind == Tok::Punct(",") {
                    top.3 = Some(i);
                } else if t.text(i) == "=>" && matches!(top.1, Ctx::ArrayLit | Ctx::ArrayCall) {
                    // the item must start its line, with its `=>` on that line
                    let item_start = top.3.map_or(top.0 + 1, |c| c + 1);
                    let first = (item_start..i).find(|&j| toks[j].kind != Tok::Comment).unwrap_or(i);
                    if t.first_on_line(first)
                        && t.line_of(toks[first].span.start as usize) == t.line_of(toks[i].span.start as usize)
                    {
                        top.2.push(i);
                    }
                }
            }
        }
    }

    // consecutive one-line statements `x = …;` / `const X = …;`
    if (o.ALIGN_ASSIGNMENTS || o.ALIGN_CLASS_CONSTANTS) && !comments {
        // (line, operator token, is a constant, indent column) for each line holding one
        let mut found: Vec<(usize, usize, bool, usize)> = Vec::new();
        let mut i = 0;
        while i < toks.len() {
            let tk = &toks[i];
            if matches!(tk.kind, Tok::Comment | Tok::OpenTag | Tok::Eof) || !t.first_on_line(i) {
                i += 1;
                continue;
            }
            // a statement start: after `;`, `{`, `}` or at the top
            let starts = t.prev_sig(i).is_none_or(|p| matches!(t.text(p), ";" | "{" | "}"));
            let line = t.line_of(tk.span.start as usize);
            if starts {
                let is_const = t.text(i).eq_ignore_ascii_case("const")
                    || (i + 1 < toks.len() && t.text(i + 1).eq_ignore_ascii_case("const") && tk.kind == Tok::Ident);
                // the first assignment operator outside brackets, and the `;` on the same line
                let mut depth = 0i32;
                let mut op = None;
                let mut j = i;
                let mut ends_here = false;
                while j < toks.len() && t.line_of(toks[j].span.start as usize) == line {
                    match t.text(j) {
                        "(" | "[" | "{" => depth += 1,
                        ")" | "]" | "}" => depth -= 1,
                        ";" if depth == 0 => {
                            ends_here = true;
                            break;
                        }
                        x if depth == 0
                            && op.is_none()
                            && ASSIGN.contains(&x)
                            && matches!(toks[j].kind, Tok::Punct(_)) =>
                        {
                            op = Some(j)
                        }
                        _ => {}
                    }
                    j += 1;
                }
                if let (Some(op), true) = (op, ends_here) {
                    let wanted = if is_const { o.ALIGN_CLASS_CONSTANTS } else { o.ALIGN_ASSIGNMENTS };
                    if wanted {
                        found.push((line, op, is_const, o.width(&src[t.line_starts[line]..tk.span.start as usize])));
                    }
                }
            }
            i += 1;
        }
        // groups: consecutive lines, same kind and indentation
        let mut group: Vec<usize> = Vec::new();
        let mut last: Option<(usize, bool, usize)> = None;
        for &(line, op, is_const, indent) in &found {
            if last.is_some_and(|(l, c, ind)| l + 1 == line && c == is_const && ind == indent) {
                group.push(op);
            } else {
                align_group(&t, &group, &mut edits);
                group = vec![op];
            }
            last = Some((line, is_const, indent));
        }
        align_group(&t, &group, &mut edits);
    }

    // trailing `//` comments of consecutive lines
    if o.ALIGN_INLINE_COMMENTS && comments {
        let mut group: Vec<usize> = Vec::new();
        let mut last_line = None;
        for i in 1..toks.len() {
            let tk = &toks[i];
            let text = t.text(i);
            let trailing = tk.kind == Tok::Comment
                && (text.starts_with("//") || text.starts_with('#'))
                && !t.first_on_line(i)
                && !matches!(toks[i - 1].kind, Tok::Comment | Tok::InlineHtml | Tok::OpenTag);
            if !trailing {
                continue;
            }
            let line = t.line_of(tk.span.start as usize);
            if last_line.is_some_and(|l: usize| l + 1 == line) {
                group.push(i);
            } else {
                align_targets(&t, &group, false, &mut edits);
                group = vec![i];
            }
            last_line = Some(line);
        }
        align_targets(&t, &group, false, &mut edits);
    }
    edits.sort_by_key(|e| e.start);
    edits.dedup_by_key(|e| e.start);
    edits
}
