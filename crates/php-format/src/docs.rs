//! PHPDoc comments: the text inside `/** … */` (and tabs in `/* … */`).
//!
//! - tabs become `TAB_SIZE` spaces (unless indenting with tabs);
//! - `@param`, `@var`, `@property` lines get `PHPDOC_PARAM_SPACES_*` spaces between tag, type,
//!   name and description; with `ALIGN_PHPDOC_PARAM_NAMES` / `ALIGN_PHPDOC_COMMENTS` the names /
//!   descriptions of the `@param` lines line up;
//! - tags are sorted by their `*_WEIGHT` (`SORT_PHPDOC_ELEMENTS`): `@param` before `@return`
//!   before `@throws` before the rest, each moving with its continuation and blank lines, the
//!   description on top staying where it is;
//! - `PHPDOC_BLANK_LINE_BEFORE_TAGS`, `PHPDOC_BLANK_LINES_AROUND_PARAMETERS`,
//!   `PHPDOC_KEEP_BLANK_LINES`.
//!
//! Only comment text changes. [`verify`] checks that every other token is unchanged and that each
//! comment still has the same lines, up to whitespace and order.

use crate::options::FormatOptions;
use php_syntax::lexer::{lex_with_trivia, Tok, Token};

/// Whitespace inside doc comments: spaces and tabs only (a no-break space is text).
fn ws(c: char) -> bool {
    c == ' ' || c == '\t' || c == '\r'
}

const PARAM_LIKE: &[&str] = &[
    "@param",
    "@var",
    "@property",
    "@property-read",
    "@property-write",
    "@phpstan-param",
    "@psalm-param",
    "@phpstan-var",
    "@psalm-var",
];

/// One line inside a multi-line doc comment.
#[derive(Clone, Debug)]
struct DocLine {
    /// Whitespace and `*` in front of the content, as written (`     * `).
    prefix: String,
    /// The text after the prefix.
    content: String,
    /// The line has a leading `*`.
    starred: bool,
}

impl DocLine {
    fn blank(&self) -> bool {
        self.content.trim_matches(ws).is_empty()
    }

    fn tag(&self) -> Option<&str> {
        let c = self.content.trim_start_matches(ws);
        c.starts_with('@').then(|| c.split(|ch: char| ws(ch)).next().unwrap_or(c))
    }

    fn render(&self) -> String {
        if self.blank() {
            self.prefix.trim_end_matches(ws).to_string()
        } else {
            format!("{}{}", self.prefix, self.content)
        }
    }
}

/// `@param` line parts.
struct Param<'a> {
    /// What follows the type as written (used when there is no name).
    raw_rest: &'a str,
    tag: &'a str,
    ty: &'a str,
    name: &'a str,
    desc: &'a str,
}

fn split_ws(s: &str) -> (&str, &str) {
    let t = s.trim_start_matches(ws);
    let end = t.find(ws).unwrap_or(t.len());
    (&t[..end], &t[end..])
}

fn is_name(s: &str) -> bool {
    s.starts_with('$') || s.starts_with("&$") || s.starts_with("...$") || s.starts_with("&...$")
}

/// Split a `@param`-like line: `@param type $name description`.
fn parse_param(content: &str) -> Option<Param<'_>> {
    let c = content.trim_start_matches(ws);
    let (tag, rest) = split_ws(c);
    if !PARAM_LIKE.iter().any(|t| tag.eq_ignore_ascii_case(t)) {
        return None;
    }
    let rest = rest.trim_start_matches(ws);
    // the type: up to whitespace outside brackets (`array<string, int>`, `array{a: int}`)
    let mut depth = 0i32;
    let mut end = rest.len();
    for (i, ch) in rest.char_indices() {
        match ch {
            '<' | '{' | '(' | '[' => depth += 1,
            '>' | '}' | ')' | ']' => depth -= 1,
            c if ws(c) && depth <= 0 => {
                end = i;
                break;
            }
            _ => {}
        }
    }
    let (mut ty, mut rest) = (&rest[..end], &rest[end..]);
    let mut name = "";
    if is_name(ty) {
        name = ty;
        ty = "";
    } else {
        let r = rest.trim_start_matches(ws);
        if is_name(r) {
            // `$name` ends where a name cannot go on (`$envKey|null` → `$envKey`)
            let skip = r.find('$').unwrap_or(0) + 1;
            let len = r[skip..]
                .find(|ch: char| !(ch.is_alphanumeric() || ch == '_' || (ch as u32) >= 0x80))
                .unwrap_or(r.len() - skip);
            name = &r[..skip + len];
            rest = &r[skip + len..];
        }
    }
    if name.is_empty() && ty.is_empty() {
        return None;
    }
    Some(Param { raw_rest: rest, tag, ty, name, desc: rest.trim_matches(ws) })
}

fn spaces(n: usize) -> String {
    " ".repeat(n.max(1))
}

fn param_line(
    p: &Param<'_>,
    o: &FormatOptions,
    ty_w: usize,
    name_w: usize,
    align_names: bool,
    align_desc: bool,
) -> String {
    let mut out = p.tag.to_string();
    // the PHPDOC_PARAM_SPACES_* options are for `@param`; `@var` and the rest get one space
    let is_param = p.tag.eq_ignore_ascii_case("@param");
    let one = |n: usize| if is_param { n } else { 1 };
    // without a `$name` only the space after the tag is set (PhpStorm leaves the rest)
    if p.name.is_empty() {
        out += &spaces(one(o.PHPDOC_PARAM_SPACES_BETWEEN_TAG_AND_TYPE));
        out += p.ty;
        out += p.raw_rest;
        return out.trim_end_matches(ws).to_string();
    }
    let tag_type = one(o.PHPDOC_PARAM_SPACES_BETWEEN_TAG_AND_TYPE);
    let type_name = one(o.PHPDOC_PARAM_SPACES_BETWEEN_TYPE_AND_NAME);
    let name_desc = one(o.PHPDOC_PARAM_SPACES_BETWEEN_NAME_AND_DESCRIPTION);
    if !p.ty.is_empty() {
        out += &spaces(tag_type);
        out += p.ty;
    }
    if !p.name.is_empty() {
        let pad = if align_names { ty_w - p.ty.chars().count() } else { 0 };
        out += &spaces(if p.ty.is_empty() { tag_type } else { type_name + pad });
        out += p.name;
    }
    if !p.desc.is_empty() {
        let mut pad = 0;
        if align_desc {
            pad = name_w - p.name.chars().count();
            if align_names && p.name.is_empty() {
                pad += ty_w - p.ty.chars().count();
            }
        }
        out += &spaces(name_desc + pad);
        out += p.desc;
    }
    out
}

/// Tags PhpStorm knows (and has a weight for). A doc comment with any other tag is not sorted.
const KNOWN_TAGS: &[&str] = &[
    "@param",
    "@return",
    "@throws",
    "@author",
    "@link",
    "@internal",
    "@uses",
    "@version",
    "@api",
    "@category",
    "@copyright",
    "@deprecated",
    "@example",
    "@filesource",
    "@global",
    "@ignore",
    "@license",
    "@method",
    "@package",
    "@property",
    "@property-read",
    "@property-write",
    "@see",
    "@since",
    "@subpackage",
    "@todo",
    "@var",
];

fn weight(tag: &str, o: &FormatOptions) -> usize {
    match tag.to_ascii_lowercase().as_str() {
        "@param" => o.PARAM_WEIGHT,
        "@return" => o.RETURN_WEIGHT,
        "@throws" => o.THROWS_WEIGHT,
        "@author" => o.AUTHOR_WEIGHT,
        "@link" => o.LINK_WEIGHT,
        "@internal" => o.INTERNAL_WEIGHT,
        "@uses" => o.USES_WEIGHT,
        "@version" => o.VERSION_WEIGHT,
        "@api" => o.API_WEIGHT,
        "@category" => o.CATEGORY_WEIGHT,
        "@copyright" => o.COPYRIGHT_WEIGHT,
        "@deprecated" => o.DEPRECATED_WEIGHT,
        "@example" => o.EXAMPLE_WEIGHT,
        "@filesource" => o.FILESOURCE_WEIGHT,
        "@global" => o.GLOBAL_WEIGHT,
        "@ignore" => o.IGNORE_WEIGHT,
        "@license" => o.LICENSE_WEIGHT,
        "@method" => o.METHOD_WEIGHT,
        "@package" => o.PACKAGE_WEIGHT,
        "@property" => o.PROPERTY_WEIGHT,
        "@property-read" => o.PROPERTY_READ_WEIGHT,
        "@property-write" => o.PROPERTY_WRITE_WEIGHT,
        "@see" => o.SEE_WEIGHT,
        "@since" => o.SINCE_WEIGHT,
        "@subpackage" => o.SUBPACKAGE_WEIGHT,
        "@todo" => o.TODO_WEIGHT,
        "@var" => o.VAR_WEIGHT,
        _ => o.UNKNOWN_TAG_WEIGHT,
    }
}

fn untab(s: &str, o: &FormatOptions) -> String {
    if o.USE_TAB_CHARACTER || !s.contains('\t') {
        s.to_string()
    } else {
        s.replace('\t', &" ".repeat(o.TAB_SIZE))
    }
}

/// The new text of doc comment `text`, None when unchanged.
fn doc_comment(text: &str, o: &FormatOptions) -> Option<String> {
    let text = text.replace("\r\n", "\n");
    if !text.contains('\n') {
        // `/** @var int $x */`
        let inner = text.strip_prefix("/**")?.strip_suffix("*/")?;
        if inner.trim_matches(ws).is_empty() {
            return None;
        }
        let lead = if inner.starts_with(ws) { " " } else { "" };
        let trail = if inner.ends_with(ws) { " " } else { "" };
        let body = untab(inner.trim_matches(ws), o);
        let body = match parse_param(&body) {
            Some(p) => param_line(&p, o, 0, 0, false, false),
            None => body,
        };
        return Some(format!("/**{lead}{body}{trail}*/"));
    }
    let raw: Vec<&str> = text.split('\n').collect();
    let (first, last) = (raw[0], raw[raw.len() - 1]);
    // the last line must be `*/` alone (with whitespace); otherwise only tabs are replaced
    if !last.trim_start_matches(ws).starts_with("*/") || !first.starts_with("/**") || raw.len() < 2 {
        return Some(untab(&text, o));
    }
    let mut lines: Vec<DocLine> = Vec::new();
    for l in &raw[1..raw.len() - 1] {
        let t = l.trim_start_matches(ws);
        let ws = &l[..l.len() - t.len()];
        if let Some(after) = t.strip_prefix('*') {
            let content = after.strip_prefix(' ').unwrap_or(after);
            let prefix = format!("{ws}*{}", if after.starts_with(' ') || after.is_empty() { " " } else { "" });
            lines.push(DocLine { prefix, content: untab(content, o), starred: true });
        } else {
            lines.push(DocLine { prefix: ws.to_string(), content: untab(t, o), starred: false });
        }
    }
    let first_text = untab(first, o);
    let well_formed = lines.iter().all(|l| l.starred) && first.trim_matches(ws) == "/**";

    // spaces in @param / @var lines (aligned over the @param lines when asked)
    let align_names = o.ALIGN_PHPDOC_PARAM_NAMES;
    let align_desc = o.ALIGN_PHPDOC_COMMENTS;
    let (mut ty_w, mut name_w) = (0, 0);
    for l in &lines {
        if let Some(p) = parse_param(&l.content).filter(|p| p.tag.eq_ignore_ascii_case("@param")) {
            ty_w = ty_w.max(p.ty.chars().count());
            name_w = name_w.max(p.name.chars().count());
        }
    }
    for l in lines.iter_mut() {
        if let Some(p) = parse_param(&l.content) {
            let is_param = p.tag.eq_ignore_ascii_case("@param");
            l.content = param_line(&p, o, ty_w, name_w, align_names && is_param, align_desc && is_param);
        }
    }

    if well_formed {
        // split into the description and tag elements (tag line + continuation + blank lines)
        let first_tag = lines.iter().position(|l| l.tag().is_some()).unwrap_or(lines.len());
        let mut description: Vec<DocLine> = lines[..first_tag].to_vec();
        let mut elements: Vec<Vec<DocLine>> = Vec::new();
        for l in &lines[first_tag..] {
            if l.tag().is_some() {
                elements.push(vec![l.clone()]);
            } else if let Some(e) = elements.last_mut() {
                e.push(l.clone());
            }
        }
        // like PhpStorm: not when a tag is unknown to it, or a `@see` / `@link` holds a URL
        let sortable = elements.iter().all(|e| {
            let tag = e[0].tag().unwrap_or("").to_ascii_lowercase();
            KNOWN_TAGS.contains(&tag.as_str())
                && !(matches!(tag.as_str(), "@see" | "@link") && e[0].content.contains("://"))
        });
        if o.SORT_PHPDOC_ELEMENTS && sortable {
            elements.sort_by_key(|e| weight(e[0].tag().unwrap_or(""), o));
        }
        let blank_like = |l: &DocLine| DocLine { prefix: l.prefix.clone(), content: String::new(), starred: true };
        if !o.PHPDOC_KEEP_BLANK_LINES {
            description.retain(|l| !l.blank());
            for e in elements.iter_mut() {
                e.retain(|l| !l.blank());
            }
        }
        // a blank line between the description and the tags
        if o.PHPDOC_BLANK_LINE_BEFORE_TAGS
            && !elements.is_empty()
            && description.iter().any(|l| !l.blank())
            && description.last().is_some_and(|l| !l.blank())
        {
            let b = blank_like(&elements[0][0]);
            description.push(b);
        }
        // blank lines around the @param group
        if o.PHPDOC_BLANK_LINES_AROUND_PARAMETERS {
            let is_param = |e: &Vec<DocLine>| e[0].tag().is_some_and(|t| t.eq_ignore_ascii_case("@param"));
            if let (Some(first), Some(last)) = (elements.iter().position(is_param), elements.iter().rposition(is_param))
            {
                if last + 1 < elements.len() && elements[last].last().is_some_and(|l| !l.blank()) {
                    let b = blank_like(&elements[last][0]);
                    elements[last].push(b);
                }
                if first > 0 && elements[first - 1].last().is_some_and(|l| !l.blank()) {
                    let b = blank_like(&elements[first][0]);
                    elements[first - 1].push(b);
                }
            }
        }
        lines = description.into_iter().chain(elements.into_iter().flatten()).collect();
    }
    let mut out = first_text;
    for l in &lines {
        out.push('\n');
        out.push_str(&l.render());
    }
    out.push('\n');
    out.push_str(last);
    Some(out)
}

/// Doc comments rewritten; None when nothing changes.
pub(crate) fn rewrite(src: &str, toks: &[Token], o: &FormatOptions) -> Option<String> {
    let template = toks.iter().any(|t| {
        t.kind == Tok::InlineHtml
            && !t.span.text(src).trim().is_empty()
            && !(t.span.start == 0 && t.span.text(src).starts_with("#!"))
    });
    if template {
        return None;
    }
    let mut out = String::with_capacity(src.len());
    let mut at = 0;
    let mut changed = false;
    for t in toks.iter().filter(|t| t.kind == Tok::Comment) {
        let text = t.span.text(src);
        let new = if text.starts_with("/**") && text != "/**/" {
            doc_comment(text, o)
        } else if text.starts_with("/*") && text.contains('\n') && text.contains('\t') && !o.USE_TAB_CHARACTER {
            Some(untab(text, o))
        } else {
            None
        };
        let Some(new) = new else { continue };
        // keep the file's line breaks
        let new = if text.contains("\r\n") { new.replace('\n', "\r\n") } else { new };
        if new == text {
            continue;
        }
        out.push_str(&src[at..t.span.start as usize]);
        out.push_str(&new);
        at = t.span.end as usize;
        changed = true;
    }
    if !changed {
        return None;
    }
    out.push_str(&src[at..]);
    Some(out)
}

/// A comment's lines without any whitespace, empty ones dropped, sorted.
fn comment_key(text: &str) -> Vec<String> {
    let mut v: Vec<String> = text
        .lines()
        .map(|l| l.chars().filter(|c| !ws(*c)).collect::<String>())
        .filter(|l| !l.is_empty() && l != "*")
        .collect();
    v.sort();
    v
}

/// Same tokens; comments with the same lines up to whitespace and order.
pub(crate) fn verify(before: &str, after: &str) -> bool {
    let (a, b) = (lex_with_trivia(before), lex_with_trivia(after));
    a.len() == b.len()
        && a.iter().zip(&b).all(|(x, y)| {
            let (sx, sy) = (x.span.text(before), y.span.text(after));
            x.kind == y.kind
                && if x.kind == Tok::Comment { sx == sy || comment_key(sx) == comment_key(sy) } else { sx == sy }
        })
}
