//! Wrapping: line breaks inside argument / parameter lists, arrays and `->` chains. Runs on
//! already formatted text (so line widths are real) and is repeated until nothing changes:
//!
//! - a list that spans several lines (it was wrapped by hand, or something inside it was) gets
//!   every item on its own line when its option says "chop down" (`*_WRAP` = 5 or 2), with `(`
//!   and `)` on their own lines per `*_LPAREN_ON_NEXT_LINE` / `*_RPAREN_ON_NEXT_LINE`; line breaks
//!   inside a closure or an array item do not count (`TREAT_MULTILINE_ARRAYS_AND_LAMBDAS_MULTILINE`);
//! - a line longer than `RIGHT_MARGIN` (trailing comment included) is broken at the last place
//!   before the overflow that allows it: an item of a list, or a `->` of a chain
//!   (`METHOD_CALL_CHAIN_WRAP`);
//! - `*_WRAP` = 2 wraps every item always.
//!
//! Only line breaks are added; indentation comes from the indentation pass run afterwards.

use crate::options::FormatOptions;
use crate::space::{bracket_contexts, Ctx, Edit};
use php_syntax::lexer::{Tok, Token};

const AS_NEEDED: usize = 1;
const ALWAYS: usize = 2;
const ON_EVERY_ITEM: usize = 4;

/// A wrappable list: `(…)` of a call or declaration, `[…]` / `array(…)`.
struct List {
    open: usize,
    close: usize,
    /// First token of every item.
    items: Vec<usize>,
    /// Last token of every item.
    item_ends: Vec<usize>,
    wrap: usize,
    lparen: bool,
    rparen: bool,
}

struct Wrapper<'a> {
    o: &'a FormatOptions,
    lists: Vec<List>,
    /// `->` / `?->` tokens that may take a line break before them, with the last token of
    /// their link.
    arrows: Vec<(usize, usize)>,
    /// For the opening token of a closure body, `match` body or array: its closing token (line
    /// breaks inside do not make an enclosing list multi-line); `usize::MAX` elsewhere.
    lambda_end: Vec<usize>,
    /// For each token: the opener of the innermost closure body / array around it (not one
    /// starting at the token itself); `usize::MAX` when none.
    in_lambda: Vec<usize>,
    /// Start line of every token, end line of every token.
    start_line: Vec<usize>,
    end_line: Vec<usize>,
}

impl Wrapper<'_> {
    fn first_on_line(&self, i: usize) -> bool {
        i == 0 || self.end_line[i - 1] < self.start_line[i]
    }

    /// Is there a line break at one of the list's own separators: after `(`, after a comma, before
    /// `)`? (Breaks inside an item, like a wrapped operand, do not count.)
    fn broken(&self, l: &List) -> bool {
        let breaks_before = |i: usize| i > 0 && self.end_line[i - 1] < self.start_line[i];
        l.items.iter().any(|&i| breaks_before(i)) || breaks_before(l.close)
    }

    /// Does the list span lines: broken at its own separators, or holding a broken list (not
    /// counting those inside closures and arrays, unless `TREAT_MULTILINE_ARRAYS_AND_LAMBDAS_MULTILINE`)?
    fn multiline(&self, li: usize) -> bool {
        let l = &self.lists[li];
        if self.broken(l) {
            return true;
        }
        // lists inside, in token order after this one
        let mut skip_to = 0;
        for inner in &self.lists[li + 1..] {
            if inner.open > l.close {
                break;
            }
            if inner.open < skip_to {
                continue;
            }
            if !self.o.TREAT_MULTILINE_ARRAYS_AND_LAMBDAS_MULTILINE {
                // a closure / array somewhere in between hides what is inside it
                let around = self.in_lambda[inner.open];
                if around != usize::MAX && around > l.open {
                    skip_to = self.lambda_end[around];
                    continue;
                }
                if self.lambda_end[inner.open] != usize::MAX {
                    skip_to = inner.close;
                    continue;
                }
            }
            if inner.wrap & (ON_EVERY_ITEM | ALWAYS) != 0 && self.broken(inner) {
                return true;
            }
        }
        false
    }

    /// Line breaks for list `l` wrapped item by item.
    fn chop(&self, l: &List, out: &mut Vec<usize>) {
        for (n, &item) in l.items.iter().enumerate() {
            if n == 0 && !l.lparen {
                continue;
            }
            out.push(item);
        }
        if l.rparen {
            out.push(l.close);
        }
    }
}

/// Is the block `{` at `open` the body of a closure, an anonymous class or a `match`? Walks back
/// over the header (`function (…) use (…): T`, `match (…)`, `class (…) extends A`).
fn lambda_block(toks: &[Token], src: &str, open: usize) -> bool {
    let mut j = open;
    let mut depth = 0i32;
    while j > 0 {
        j -= 1;
        if toks[j].kind == Tok::Comment {
            continue;
        }
        let tx = toks[j].span.text(src);
        match tx {
            ")" | "]" => depth += 1,
            "(" | "[" => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            _ if depth > 0 => {}
            ";" | "{" | "}" | "=" | "," | "=>" => return false,
            _ if ["function", "fn", "class", "match"].iter().any(|k| tx.eq_ignore_ascii_case(k)) => return true,
            _ => {}
        }
    }
    false
}

/// The token that a line break before item token `i` goes in front of: a trailing `//` comment
/// right after the comma stays on the comma's line.
fn item_start(toks: &[Token], src: &str, mut i: usize, close: usize) -> Option<usize> {
    while i < close && toks[i].kind == Tok::Comment {
        let text = toks[i].span.text(src);
        let same_line = !src[toks[i - 1].span.end as usize..toks[i].span.start as usize].contains('\n');
        if same_line && (text.starts_with("//") || text.starts_with('#')) {
            i += 1;
        } else {
            break;
        }
    }
    (i < close).then_some(i)
}

/// Line breaks to add to formatted `src` (empty when nothing needs wrapping).
pub(crate) fn wrap(src: &str, toks: &[Token], o: &FormatOptions) -> Vec<Edit> {
    let mut line_starts = vec![0usize];
    line_starts.extend(src.match_indices('\n').map(|(i, _)| i + 1));
    let line_of = |off: usize| line_starts.partition_point(|&s| s <= off) - 1;
    let start_line: Vec<usize> = toks.iter().map(|t| line_of(t.span.start as usize)).collect();
    let end_line: Vec<usize> =
        toks.iter().map(|t| line_of((t.span.end as usize).max(t.span.start as usize + 1) - 1)).collect();

    // brackets with their contexts, matched
    let ctxs = bracket_contexts(src, toks, o);
    let mut lists = Vec::new();
    let mut lambdas = Vec::new();
    let mut stack: Vec<(usize, Ctx)> = Vec::new();
    // closers matched to openers, and top-level commas of each open bracket
    let mut commas: Vec<Vec<usize>> = Vec::new();
    let mut ci = 0;
    for (i, t) in toks.iter().enumerate() {
        if ci < ctxs.len() && ctxs[ci].0 == i {
            let ctx = ctxs[ci].1;
            ci += 1;
            match t.kind {
                Tok::Punct("(" | "[" | "#[" | "{") => {
                    stack.push((i, ctx));
                    commas.push(Vec::new());
                }
                _ => {
                    let Some((open, ctx)) = stack.pop() else { continue };
                    let cs = commas.pop().unwrap_or_default();
                    if matches!(ctx, Ctx::ArrayLit | Ctx::ArrayCall | Ctx::BraceExpr)
                        || ctx == Ctx::Block && lambda_block(toks, src, open)
                    {
                        lambdas.push((open, i));
                    }
                    // `isset()`, `empty()`, `unset()`, `exit()` … are not calls
                    let construct = toks[..open].iter().rev().find(|t| t.kind != Tok::Comment).is_some_and(|t| {
                        let w = t.span.text(src);
                        t.kind == Tok::Ident
                            && ["isset", "empty", "unset", "exit", "die", "eval", "list"]
                                .iter()
                                .any(|k| w.eq_ignore_ascii_case(k))
                    });
                    let (wrap, lparen, rparen) = match ctx {
                        Ctx::Call if construct => continue,
                        Ctx::Call | Ctx::Attr if t.span.text(src) == ")" => (
                            o.CALL_PARAMETERS_WRAP,
                            o.CALL_PARAMETERS_LPAREN_ON_NEXT_LINE,
                            o.CALL_PARAMETERS_RPAREN_ON_NEXT_LINE,
                        ),
                        Ctx::Decl | Ctx::ClosureDecl => (
                            o.METHOD_PARAMETERS_WRAP,
                            o.METHOD_PARAMETERS_LPAREN_ON_NEXT_LINE,
                            o.METHOD_PARAMETERS_RPAREN_ON_NEXT_LINE,
                        ),
                        Ctx::ArrayLit | Ctx::ArrayCall => (
                            o.ARRAY_INITIALIZER_WRAP,
                            o.ARRAY_INITIALIZER_LBRACE_ON_NEXT_LINE,
                            o.ARRAY_INITIALIZER_RBRACE_ON_NEXT_LINE,
                        ),
                        _ => continue,
                    };
                    // `f(...)`: a first-class callable
                    let callable = toks[open + 1..i]
                        .iter()
                        .filter(|t| t.kind != Tok::Comment)
                        .map(|t| t.span.text(src))
                        .eq(["..."]);
                    if wrap == 0 || i == open + 1 || callable {
                        continue;
                    }
                    let mut items = Vec::new();
                    let mut item_ends = Vec::new();
                    let ends = cs.iter().map(|&c| c - 1).chain(std::iter::once(i - 1));
                    for (start, end) in std::iter::once(open + 1).chain(cs.iter().map(|&c| c + 1)).zip(ends) {
                        if let Some(s) = item_start(toks, src, start, i) {
                            if s <= end {
                                items.push(s);
                                item_ends.push(end);
                            }
                        }
                    }
                    if !items.is_empty() {
                        lists.push(List { open, close: i, items, item_ends, wrap, lparen, rparen });
                    }
                }
            }
        } else if t.kind == Tok::Punct(",") {
            if let Some(c) = commas.last_mut() {
                c.push(i);
            }
        }
    }
    // closing brackets were collected in close order; keep lists sorted by their opener
    lists.sort_by_key(|l| l.open);
    let mut lambda_end = vec![usize::MAX; toks.len()];
    for (open, close) in lambdas {
        lambda_end[open] = close;
    }

    // `->` that may wrap: after a method call (`a()->b()`), not before the first call of a chain
    // (unless allowed); with the end of the link (`->name(…)[…]`)
    let mut arrows = Vec::new();
    if o.METHOD_CALL_CHAIN_WRAP != 0 {
        let sig: Vec<usize> =
            (0..toks.len()).filter(|&i| !matches!(toks[i].kind, Tok::Comment | Tok::OpenTag)).collect();
        let text = |k: usize| toks[sig[k]].span.text(src);
        // index in `sig` of the opener matching the closer at `k`
        let opener = |k: usize| -> Option<usize> {
            let mut depth = 0i32;
            let mut j = k;
            loop {
                match text(j) {
                    ")" | "]" => depth += 1,
                    "(" | "[" => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(j);
                        }
                    }
                    _ => {}
                }
                j = j.checked_sub(1)?;
            }
        };
        for k in 1..sig.len() {
            if !matches!(text(k), "->" | "?->") {
                continue;
            }
            // what is in front: `->name(…)` / `::name(…)` (a method call), skipping `[…]` after it
            let mut j = k - 1;
            while text(j) == "]" {
                match opener(j).and_then(|o| o.checked_sub(1)) {
                    Some(p) => j = p,
                    None => break,
                }
            }
            let after_call = text(j) == ")"
                && opener(j).is_some_and(|op| {
                    op >= 2
                        && matches!(toks[sig[op - 1]].kind, Tok::Ident)
                        && matches!(text(op - 2), "->" | "?->" | "::")
                });
            if !(after_call || o.WRAP_FIRST_METHOD_IN_CALL_CHAIN) {
                continue;
            }
            // the end of this link
            let mut e = k + 1;
            if e < sig.len() && matches!(toks[sig[e]].kind, Tok::Ident | Tok::Variable) {
                while e + 1 < sig.len() && matches!(text(e + 1), "(" | "[") {
                    let mut depth = 0i32;
                    let mut m = e + 1;
                    while m < sig.len() {
                        match text(m) {
                            "(" | "[" => depth += 1,
                            ")" | "]" => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                        m += 1;
                    }
                    e = m.min(sig.len() - 1);
                }
            }
            arrows.push((sig[k], sig[e.min(sig.len() - 1)]));
        }
    }

    // the innermost closure / array around each token, in one sweep
    let mut in_lambda = vec![usize::MAX; toks.len()];
    let mut open_lambdas: Vec<usize> = Vec::new();
    for (i, slot) in in_lambda.iter_mut().enumerate() {
        while open_lambdas.last().is_some_and(|&o| lambda_end[o] < i) {
            open_lambdas.pop();
        }
        *slot = open_lambdas.last().copied().unwrap_or(usize::MAX);
        if lambda_end[i] != usize::MAX {
            open_lambdas.push(i);
        }
    }
    let w = Wrapper { o, lists, arrows, lambda_end, in_lambda, start_line, end_line };
    let mut breaks: Vec<usize> = Vec::new();

    // lists that span lines (or always wrap) get chopped
    for (li, l) in w.lists.iter().enumerate() {
        let chop = l.wrap & (ON_EVERY_ITEM | ALWAYS) != 0;
        if chop && (l.wrap & ALWAYS != 0 || w.multiline(li)) {
            w.chop(l, &mut breaks);
        }
    }

    // lines that are too long: break at the last candidate before the overflow
    let margin = o.RIGHT_MARGIN;
    if margin > 0 {
        let mut first_tok_of_line = vec![usize::MAX; line_starts.len()];
        for (i, t) in toks.iter().enumerate() {
            if t.kind == Tok::Eof {
                continue;
            }
            let ln = w.start_line[i];
            if first_tok_of_line[ln] == usize::MAX {
                first_tok_of_line[ln] = i;
            }
        }
        // what can happen at each token, in token order
        let mut at_open: Vec<(usize, usize)> = Vec::new(); // (open token, list)
        let mut cands: Vec<(usize, Cand)> = Vec::new(); // fallback candidates
        let mut needed: Vec<(usize, usize)> = Vec::new(); // (token, last token): "where needed" points
        for (li, l) in w.lists.iter().enumerate() {
            if l.wrap & (ON_EVERY_ITEM | ALWAYS) != 0 {
                at_open.push((l.open, li));
            } else if l.wrap & AS_NEEDED != 0 {
                // the first item stays after `(` unless `(` gets a line of its own
                for (n, &item) in l.items.iter().enumerate().skip(usize::from(!l.lparen)) {
                    needed.push((item, l.item_ends[n]));
                }
            }
            for (n, &item) in l.items.iter().enumerate() {
                if n == 0 && !l.lparen && l.wrap & (ON_EVERY_ITEM | ALWAYS) == 0 {
                    continue;
                }
                cands.push((item, Cand::Item(li)));
            }
        }
        for &(a, end) in &w.arrows {
            needed.push((a, end));
            cands.push((a, Cand::Arrow));
        }
        at_open.sort_unstable();
        needed.sort_unstable();
        cands.sort_by_key(|c| c.0);
        for (ln, &ls) in line_starts.iter().enumerate() {
            let end = line_starts.get(ln + 1).map_or(src.len(), |&e| e - 1);
            let line = src[ls..end].trim_end_matches('\r');
            if o.width(line) <= margin {
                continue;
            }
            let first = first_tok_of_line[ln];
            if first == usize::MAX {
                continue;
            }
            let mut last_tok = first;
            while last_tok + 1 < toks.len() && w.start_line[last_tok + 1] == ln && toks[last_tok + 1].kind != Tok::Eof {
                last_tok += 1;
            }
            // start and end column of every token on the line, in one pass (a line can be
            // megabytes long: no measuring from the line start per token)
            let tab = o.TAB_SIZE.max(1);
            let adv =
                |col: usize, s: &str| s.chars().fold(col, |w, c| if c == '\t' { (w / tab + 1) * tab } else { w + 1 });
            let mut cols: Vec<(usize, usize)> = Vec::with_capacity(last_tok + 1 - first);
            let (mut col, mut at) = (0usize, ls);
            for t in &toks[first..=last_tok] {
                let (s0, e0) = (t.span.start as usize, (t.span.end as usize).min(end));
                col = adv(col, &src[at..s0]);
                let start_col = col;
                col = adv(col, &src[s0..e0.max(s0)]);
                cols.push((start_col, col));
                at = e0.max(s0);
            }
            let start_col = |i: usize| cols[i - first].0;
            let width_to = |_: usize, _: usize, i: usize| if i <= last_tok { cols[i - first].1 } else { usize::MAX };
            // left to right: the first list that does not fit, or link / item that does not fit
            let lo = at_open.partition_point(|x| x.0 < first);
            let hi = at_open.partition_point(|x| x.0 <= last_tok);
            let nlo = needed.partition_point(|x| x.0 < first);
            let nhi = needed.partition_point(|x| x.0 <= last_tok);
            let mut events: Vec<(usize, Option<usize>, usize)> = Vec::new(); // (token, list, last)
            for &(open, li) in &at_open[lo..hi] {
                let l = &w.lists[li];
                if w.start_line[l.close] == ln {
                    events.push((open, Some(li), l.close));
                }
            }
            for &(t, last) in &needed[nlo..nhi] {
                if !w.first_on_line(t) && w.end_line[last] == ln {
                    events.push((t, None, last));
                }
            }
            events.sort_unstable();
            // Fill the line: every "where needed" point whose part does not fit gets a break,
            // the columns after it moved to the new line's (estimated) indent; a list that must
            // be chopped ends this line's turn. One round handles a whole line, so the number of
            // rounds follows the nesting depth, not the number of items.
            let line_indent = o.width(&line[..line.len() - line.trim_start().len()]);
            let mut shift: isize = 0;
            let mut hit = false;
            for (t, list, last) in events {
                if width_to(ls, end, last) as isize + shift <= margin as isize {
                    continue;
                }
                match list {
                    Some(li) => {
                        if !hit {
                            w.chop(&w.lists[li], &mut breaks);
                        }
                        hit = true;
                        break;
                    }
                    None => {
                        let start = start_col(t) as isize;
                        // a break right where the line already (virtually) starts would not help
                        if start + shift <= (line_indent + o.INDENT_SIZE) as isize {
                            continue;
                        }
                        breaks.push(t);
                        hit = true;
                        shift = (line_indent + o.INDENT_SIZE) as isize - start;
                    }
                }
            }
            match hit {
                true => {}
                false => {
                    // everything fits but the line does not (a trailing comment, a long string
                    // after a list): break at the last candidate before the overflow
                    let over = (first..=last_tok).find(|&i| width_to(ls, end, i) > margin);
                    let Some(over) = over else { continue };
                    let at = cands.partition_point(|c| c.0 <= over);
                    let pick = cands[..at].iter().rev().find(|(t, _)| w.start_line[*t] == ln && !w.first_on_line(*t));
                    match pick {
                        Some(&(_, Cand::Item(li))) if w.lists[li].wrap & (ON_EVERY_ITEM | ALWAYS) != 0 => {
                            w.chop(&w.lists[li], &mut breaks)
                        }
                        Some(&(t, _)) => breaks.push(t),
                        None => {}
                    }
                }
            }
        }
    }

    breaks.sort_unstable();
    breaks.dedup();
    breaks
        .into_iter()
        .filter(|&i| i > 0 && !w.first_on_line(i))
        .map(|i| {
            let (start, end) = (toks[i - 1].span.end as usize, toks[i].span.start as usize);
            Edit { start, end, text: "\n".into() }
        })
        .collect()
}

#[derive(Clone, Copy)]
enum Cand {
    Item(usize),
    Arrow,
}
