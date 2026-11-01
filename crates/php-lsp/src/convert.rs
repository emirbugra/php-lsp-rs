use lsp_types::{Position, Range, Uri};
use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, CONTROLS};
use php_syntax::{LineIndex, Span};
use std::path::{Path, PathBuf};
use std::str::FromStr;

const PATH_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}')
    .add(b'[')
    .add(b']')
    .add(b'^')
    .add(b'|')
    .add(b'\\')
    .add(b'@')
    .add(b'+')
    .add(b'!')
    .add(b'$')
    .add(b'&')
    .add(b'\'')
    .add(b'(')
    .add(b')')
    .add(b'*')
    .add(b',')
    .add(b';')
    .add(b'=')
    .add(b':');

/// Canonical `file://` URI string for a path.
pub fn path_to_uri(p: &Path) -> String {
    let s = p.to_string_lossy();
    format!("file://{}", utf8_percent_encode(&s, PATH_SET))
}

pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let decoded = percent_decode_str(rest).decode_utf8_lossy().into_owned();
    Some(PathBuf::from(decoded))
}

/// Canonicalise whatever URI spelling the client sent.
pub fn canon(uri: &Uri) -> String {
    match uri_to_path(uri.as_str()) {
        Some(p) => path_to_uri(&p),
        None => uri.as_str().to_string(),
    }
}

pub fn lsp_uri(s: &str) -> Uri {
    Uri::from_str(s).unwrap_or_else(|_| Uri::from_str("file:///invalid").unwrap())
}

pub fn range(text: &str, lines: &LineIndex, span: Span) -> Range {
    let (sl, sc) = lines.position(text, span.start as usize);
    let (el, ec) = lines.position(text, span.end as usize);
    Range { start: Position { line: sl, character: sc }, end: Position { line: el, character: ec } }
}

pub fn offset_of(text: &str, lines: &LineIndex, pos: Position) -> usize {
    lines.offset(text, pos.line, pos.character)
}

/// Read a PHP source file: lossy UTF-8, BOM stripped (matches what editors show).
pub fn read_text(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let s = match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    };
    Some(match s.strip_prefix('\u{feff}') {
        Some(rest) => rest.to_string(),
        None => s,
    })
}
