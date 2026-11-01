//! PHP code formatter with PhpStorm-style options (PSR-12 by default).
//!
//! The formatter only ever changes whitespace between tokens: every token (and every comment)
//! is written back as it was, and [`format`] re-lexes its own output to make sure of that before
//! returning it.
//!
//! - `options.rs`: [`FormatOptions`], named like PhpStorm's options, and presets
//! - `indent.rs`: the indentation of every line, worked out from the token stream
//! - `space.rs`: spaces between tokens on a line, letter case of keywords and constants
//! - `scheme.rs`: PhpStorm code style XML (import / export)
//! - `editorconfig.rs`: `.editorconfig`, with PhpStorm's `ij_*` keys (import / export)

mod align;
mod braces;
mod breaks;
mod commas;
mod docs;
pub mod editorconfig;
mod indent;
pub mod options;
pub mod scheme;
mod space;
mod wrap;

pub use editorconfig::EditorConfig;
pub use options::{FormatOptions, PRESETS};
pub use scheme::Imported;

use indent::Action;
use php_syntax::lexer::{lex_with_trivia, Tok, Token};
use space::Edit;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatError {
    /// Brackets do not match (at this byte offset): the file has syntax errors.
    Unbalanced(usize),
    /// The result would have changed more than whitespace (a formatter bug); nothing is changed.
    Changed,
    /// The file uses `<?` short open tags. Whether that is code or text depends on the server's
    /// `short_open_tag` setting, so re-indenting it could change the page's output.
    ShortOpenTag,
}

impl std::fmt::Display for FormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FormatError::Unbalanced(at) => write!(f, "unbalanced brackets near byte {at}"),
            FormatError::Changed => write!(f, "internal check failed: the result would change more than whitespace"),
            FormatError::ShortOpenTag => write!(
                f,
                "the file uses `<?` short open tags; not formatted, since their meaning depends on the short_open_tag setting"
            ),
        }
    }
}

impl std::error::Error for FormatError {}

/// Newest PHP version, assumed when the project's is not known.
pub const LATEST_PHP: (u8, u8) = (8, 4);

/// Format a whole file (for the newest PHP version).
pub fn format(src: &str, o: &FormatOptions) -> Result<String, FormatError> {
    format_for(src, o, LATEST_PHP)
}

/// Format a whole file for PHP `php` (`(7, 4)`): trailing commas only where that PHP allows them.
pub fn format_for(src: &str, o: &FormatOptions, php: (u8, u8)) -> Result<String, FormatError> {
    let base = rewritten(src, o, 0, usize::MAX, true)?;
    let base = base.as_deref().unwrap_or(src);
    let out = with_commas(format_unchecked_lines(base, o, 0, usize::MAX)?, o, php)?;
    if !same_tokens(base, &out) {
        return Err(FormatError::Changed);
    }
    Ok(out)
}

/// Add trailing commas to lists that span lines, and lay out again (a comma can push a line
/// over the margin) until no comma is missing.
fn with_commas(mut out: String, o: &FormatOptions, php: (u8, u8)) -> Result<String, FormatError> {
    for _ in 0..4 {
        let toks = tokens(&out)?;
        let edits = commas::trailing_commas(&out, &toks, o, php);
        if edits.is_empty() {
            break;
        }
        let (with, _) = apply_edits(&out, &edits, &[]);
        out = format_unchecked_lines(&with, o, 0, usize::MAX)?;
    }
    Ok(out)
}

/// Format only lines `first..=last` (0-based); the others stay as they are. (Braces are added
/// and `else if` rewritten only when formatting the whole file.)
pub fn format_lines(src: &str, o: &FormatOptions, first: usize, last: usize) -> Result<String, FormatError> {
    let base = rewritten(src, o, first, last, true)?;
    let base = base.as_deref().unwrap_or(src);
    let out = format_unchecked_lines(base, o, first, last)?;
    if !same_tokens(base, &out) {
        return Err(FormatError::Changed);
    }
    Ok(out)
}

/// [`format`] without the final checks, for debugging the formatter itself.
#[doc(hidden)]
pub fn format_unchecked(src: &str, o: &FormatOptions) -> Result<String, FormatError> {
    let base = rewritten(src, o, 0, usize::MAX, false)?;
    with_commas(format_unchecked_lines(base.as_deref().unwrap_or(src), o, 0, usize::MAX)?, o, LATEST_PHP)
}

/// The text with braces added / `else if` rewritten and doc comments formatted (whole files
/// only), each step checked when `check`.
fn rewritten(
    src: &str,
    o: &FormatOptions,
    first: usize,
    last: usize,
    check: bool,
) -> Result<Option<String>, FormatError> {
    if first != 0 || last != usize::MAX {
        return Ok(None);
    }
    let toks = tokens(src)?;
    let braced = match braces::rewrite(src, &toks, o) {
        Some(rw) if check && !braces::verify(src, &rw) => return Err(FormatError::Changed),
        Some(rw) => Some(rw.text),
        None => None,
    };
    let base = braced.as_deref().unwrap_or(src);
    let toks = if braced.is_some() { tokens(base)? } else { toks };
    // `array(…)` → `[…]`
    let short = match commas::short_arrays(base, &toks, o) {
        Some(t) if check && !same_tokens(base, &t) => return Err(FormatError::Changed),
        Some(t) => Some(t),
        None => braced,
    };
    let base = short.as_deref().unwrap_or(src);
    let toks = tokens(base)?;
    match docs::rewrite(base, &toks, o) {
        Some(d) if check && !docs::verify(base, &d) => Err(FormatError::Changed),
        Some(d) => Ok(Some(d)),
        None => Ok(short),
    }
}

/// Most rounds of wrapping (each adds line breaks, then everything is laid out again; deeply
/// nested one-line arrays need one round per level). The loop stops as soon as nothing changes.
const WRAP_ROUNDS: usize = 100;

/// The passes: line breaks between statements, indentation and spaces, then wrapping long lines
/// (repeated with the passes before until nothing changes).
fn format_unchecked_lines(src: &str, o: &FormatOptions, first: usize, last: usize) -> Result<String, FormatError> {
    let (mut out, mut last) = layout_pass(src, o, first, last)?;
    let whole = first == 0 && last == usize::MAX;
    for _ in 0..WRAP_ROUNDS {
        let toks = tokens(&out)?;
        if toks.iter().any(|t| {
            t.kind == Tok::InlineHtml && !t.span.text(&out).trim().is_empty() && !t.span.text(&out).starts_with("#!")
        }) {
            break;
        }
        let mut edits = wrap::wrap(&out, &toks, o);
        if !whole {
            let (start, end) = line_range(&out, first, last);
            edits.retain(|e| e.start >= start && e.end <= end);
        }
        if edits.is_empty() {
            break;
        }
        let added = edits
            .iter()
            .map(|e| e.text.matches('\n').count() - out[e.start..e.end].matches('\n').count())
            .sum::<usize>();
        let (wrapped, _) = apply_edits(&out, &edits, &[]);
        if !whole {
            last += added;
        }
        (out, last) = layout_pass(&wrapped, o, first, last)?;
    }
    // aligned items take their column from the line as laid out: once more on the result
    if o.ALIGN_MULTILINE_PARAMETERS
        || o.ALIGN_MULTILINE_PARAMETERS_IN_CALLS
        || o.ALIGN_MULTILINE_ARRAY_INITIALIZER_EXPRESSION
    {
        out = layout_pass(&out, o, first, last)?.0;
    }
    Ok(out)
}

/// Line breaks between statements, then indentation and spaces. Returns the text and where the
/// last line of the range ended up.
fn layout_pass(src: &str, o: &FormatOptions, first: usize, last: usize) -> Result<(String, usize), FormatError> {
    let toks = tokens(src)?;
    // 1. line breaks (only gaps inside the range when formatting a selection)
    let (mut breaks, keep) = breaks::line_breaks(src, &toks, o)?;
    let whole = first == 0 && last == usize::MAX;
    let (range_start, range_end) = if whole { (0, src.len()) } else { line_range(src, first, last) };
    breaks.retain(|e| e.start >= range_start && e.end <= range_end);
    let added: isize = breaks
        .iter()
        .map(|e| e.text.matches('\n').count() as isize - src[e.start..e.end].matches('\n').count() as isize)
        .sum();
    let (src1, keep1) = apply_edits(src, &breaks, &keep);
    let toks1 = if breaks.is_empty() { toks } else { tokens(&src1)? };
    let last1 = if whole { last } else { (last as isize + added).max(first as isize) as usize };
    // 2. indentation and spaces
    let lines = indent::layout(&src1, &toks1, o, false, &keep1)?.lines;
    let edits = space::spacing(&src1, &toks1, o);
    let out = render(&src1, &toks1, &lines, &edits, o, first..=last1);
    // 3. alignment (whole files only)
    let mut out = out;
    if whole {
        for comments in [false, true] {
            let toks2 = tokens(&out)?;
            let aligned = align::align(&out, &toks2, o, comments);
            if !aligned.is_empty() {
                out = apply_edits(&out, &aligned, &[]).0;
            }
        }
    }
    Ok((out, last1))
}

/// Byte range of lines `first..=last` (from the start of `first` to the end of `last`).
fn line_range(src: &str, first: usize, last: usize) -> (usize, usize) {
    let mut starts = vec![0usize];
    starts.extend(src.match_indices('\n').map(|(i, _)| i + 1));
    let start = starts.get(first).copied().unwrap_or(src.len());
    let end = starts.get(last + 1).map_or(src.len(), |&s| s.saturating_sub(1));
    (start, end)
}

/// Apply sorted, non-overlapping edits; offsets in `keep` move along.
fn apply_edits(src: &str, edits: &[Edit], keep: &[(usize, usize)]) -> (String, Vec<(usize, usize)>) {
    if edits.is_empty() {
        return (src.to_string(), keep.to_vec());
    }
    let mut out = String::with_capacity(src.len() + edits.len());
    let mut at = 0;
    // (old offset, shift) after each edit
    let mut shifts: Vec<(usize, isize)> = Vec::with_capacity(edits.len());
    for e in edits {
        out.push_str(&src[at..e.start]);
        out.push_str(&e.text);
        at = e.end;
        shifts.push((e.end, out.len() as isize - at as isize));
    }
    out.push_str(&src[at..]);
    let moved = keep
        .iter()
        .map(|&(off, n)| {
            let k = shifts.partition_point(|&(end, _)| end <= off);
            let shift = if k == 0 { 0 } else { shifts[k - 1].1 };
            ((off as isize + shift) as usize, n)
        })
        .collect();
    (out, moved)
}

/// Leading whitespace line `line` (0-based) should have, also for a blank line (what a new line
/// typed there should start with). None when the line is left alone (strings, HTML). Blocks left
/// open at the end of the file are accepted, since the file is usually being typed.
pub fn line_indent(src: &str, line: usize, o: &FormatOptions) -> Result<Option<String>, FormatError> {
    let toks = tokens(src)?;
    let lines = indent::layout(src, &toks, o, true, &[])?.lines;
    Ok(match lines.get(line).map(|l| &l.action) {
        Some(Action::Indent(n)) | Some(Action::Blank { indent: n }) => Some(o.indent_str(*n)),
        _ => None,
    })
}

/// Tokens with trivia; files with short open tags are refused.
fn tokens(src: &str) -> Result<Vec<Token>, FormatError> {
    let toks = lex_with_trivia(src);
    if toks.iter().any(|t| t.kind == Tok::OpenTag && t.span.len() == 2) {
        return Err(FormatError::ShortOpenTag);
    }
    Ok(toks)
}

fn line_separator(src: &str, o: &FormatOptions) -> &'static str {
    match o.LINE_SEPARATOR.to_ascii_lowercase().as_str() {
        "\n" | "lf" => "\n",
        "\r\n" | "crlf" => "\r\n",
        _ => match src.find('\n') {
            Some(i) if i > 0 && src.as_bytes()[i - 1] == b'\r' => "\r\n",
            _ => "\n",
        },
    }
}

fn render(
    src: &str,
    toks: &[Token],
    lines: &[indent::Line],
    edits: &[Edit],
    o: &FormatOptions,
    range: std::ops::RangeInclusive<usize>,
) -> String {
    let sep = line_separator(src, o);
    let mut out = String::with_capacity(src.len() + src.len() / 8);
    let mut blanks = 0usize;
    let whole = *range.start() == 0 && *range.end() == usize::MAX;
    let mut next_edit = 0;
    let mut edited = String::new();
    for (ln, l) in lines.iter().enumerate() {
        let mut content = &src[l.start..l.end];
        // spacing edits on this line (only lines that are re-indented: not strings, HTML, comments)
        while next_edit < edits.len() && edits[next_edit].start < l.start {
            next_edit += 1;
        }
        let first_edit = next_edit;
        while next_edit < edits.len() && edits[next_edit].end <= l.end {
            next_edit += 1;
        }
        if l.code && !l.off && range.contains(&ln) && first_edit < next_edit {
            edited.clear();
            let mut at = l.start;
            for e in &edits[first_edit..next_edit] {
                edited.push_str(&src[at..e.start]);
                edited.push_str(&e.text);
                at = e.end;
            }
            edited.push_str(&src[at..l.end]);
            content = &edited;
        }
        let own_sep = &src[l.end..l.end + l.sep_len];
        if !range.contains(&ln) || l.off {
            for _ in 0..blanks {
                out.push_str(sep);
            }
            blanks = 0;
            out.push_str(content);
            out.push_str(own_sep);
            continue;
        }
        // keep a line break that belongs to a string / heredoc / HTML as written
        let line_sep = if l.end_in_token || l.sep_len == 0 { own_sep } else { sep };
        if let Action::Blank { indent } = l.action {
            blanks += 1;
            let _ = indent;
            continue;
        }
        if blanks > 0 {
            for _ in 0..blanks.min(l.blank_limit) {
                out.push_str(sep);
            }
            blanks = 0;
        }
        let strip = o.STRIP_TRAILING_SPACES && !l.end_in_token;
        let body = |s: &str| if strip { s.trim_end().to_string() } else { s.to_string() };
        match &l.action {
            Action::Verbatim => out.push_str(&body(content)),
            Action::Indent(n) => {
                out.push_str(&o.indent_str(*n));
                out.push_str(&body(content.trim_start()));
            }
            Action::CommentInner { star, delta } => {
                let rest = content.trim_start();
                if let (true, Some(star)) = (rest.starts_with('*'), star) {
                    out.push_str(&o.indent_str(*star));
                } else if !rest.is_empty() {
                    let old = o.width(&content[..content.len() - rest.len()]) as isize;
                    out.push_str(&o.indent_str((old + delta).max(0) as usize));
                }
                out.push_str(&body(rest));
            }
            Action::Blank { .. } => unreachable!(),
        }
        out.push_str(line_sep);
    }
    // blank lines at the very end
    let php_at_end = toks.iter().rev().find(|t| t.kind != Tok::Eof).is_none_or(|t| t.kind != Tok::InlineHtml);
    let end_in_range = range.contains(&lines.len().saturating_sub(1));
    if o.ENSURE_NEWLINE_AT_EOF && php_at_end && (whole || end_in_range) {
        while out.ends_with(sep) && out[..out.len() - sep.len()].ends_with(sep) {
            out.truncate(out.len() - sep.len());
        }
        if !out.is_empty() && !out.ends_with('\n') {
            out.push_str(sep);
        }
    } else {
        for _ in 0..blanks {
            out.push_str(sep);
        }
    }
    out
}

/// Tokens for comparing: (kind, text), with commas right before a closing bracket left out and
/// `array(…)` read as `[…]` (both no-ops for PHP).
fn canonical(src: &str) -> Vec<(Tok, &str)> {
    let toks = lex_with_trivia(src);
    let next_sig = |i: usize| toks[i + 1..].iter().find(|t| t.kind != Tok::Comment);
    let mut out: Vec<(Tok, &str)> = Vec::with_capacity(toks.len());
    // for each open bracket: was it an `array(`?
    let mut stack: Vec<bool> = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let t = &toks[i];
        let text = t.span.text(src);
        match (t.kind, text) {
            (Tok::Punct(","), _) if next_sig(i).is_some_and(|n| matches!(n.kind, Tok::Punct(")" | "]" | "}"))) => {}
            (Tok::Ident, w)
                if w.eq_ignore_ascii_case("array") && toks.get(i + 1).is_some_and(|n| n.kind == Tok::Punct("(")) =>
            {
                out.push((Tok::Punct("["), "["));
                stack.push(true);
                i += 1;
            }
            (Tok::Punct("(" | "[" | "{" | "#["), _) => {
                stack.push(false);
                out.push((t.kind, text));
            }
            (Tok::Punct(")"), _) if stack.pop() == Some(true) => out.push((Tok::Punct("]"), "]")),
            (Tok::Punct("]" | "}"), _) => {
                stack.pop();
                out.push((t.kind, text));
            }
            _ => out.push((t.kind, text)),
        }
        i += 1;
    }
    out
}

/// Same tokens in the same order; comments may differ only in the indentation of their lines,
/// keywords and `true` / `false` / `null` only in letter case (and see [`canonical`]).
fn same_tokens(a: &str, b: &str) -> bool {
    let (ta, tb) = (canonical(a), canonical(b));
    if ta.len() != tb.len() {
        return false;
    }
    let squash = |s: &str| s.lines().map(str::trim).collect::<Vec<_>>().join("\n");
    let mut prev = "";
    ta.iter().zip(&tb).all(|(&(kx, sx), &(ky, sy))| {
        let same = kx == ky
            && match kx {
                Tok::Comment => squash(sx) == squash(sy),
                // not a member name (`$o->NULL`, `A::TRUE` are case-sensitive)
                Tok::Ident if !matches!(prev, "->" | "?->" | "::") => {
                    sx == sy || sx.eq_ignore_ascii_case(sy) && space::case_insensitive_word(&sx.to_ascii_lowercase())
                }
                _ => sx == sy,
            };
        if kx != Tok::Comment {
            prev = sx;
        }
        same
    })
}
