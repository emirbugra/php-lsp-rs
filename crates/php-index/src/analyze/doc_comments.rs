//! Docblocks: class names inside them become references, `@var` tags give variables a type.

use super::{Analyzer, RefTarget, Reference};
use crate::docblock;
use crate::names::is_builtin_type;
use crate::types;
use php_syntax::*;

/// Byte range of `$name` on an `@var` line of a docblock.
fn doc_var_span(raw: &str, name: &str) -> Option<(usize, usize)> {
    let needle = format!("${name}");
    let mut from = 0;
    while let Some(i) = raw[from..].find("@var") {
        let line_start = from + i;
        let line_end = raw[line_start..].find('\n').map(|e| line_start + e).unwrap_or(raw.len());
        let line = &raw[line_start..line_end];
        let mut k = 0;
        while let Some(j) = line[k..].find(&needle) {
            let at = k + j;
            let end = at + needle.len();
            if !line[end..].starts_with(|c: char| c.is_alphanumeric() || c == '_') {
                return Some((line_start + at, line_start + end));
            }
            k = end;
        }
        from = line_end;
    }
    None
}

impl Analyzer<'_> {
    /// Class names written in a docblock's type positions (`@var`, `@param`, `@return`, `@throws`,
    /// `@property`, `@method`, `@mixin`, `@extends`, `@template … of`, `@see` …) become class references,
    /// so go-to-definition / hover / find-references / rename work inside comments. Only names that
    /// resolve to an indexed class are recorded (template names and pseudo types drop out naturally).
    pub(super) fn doc_refs(&mut self, doc: Option<Span>) {
        let Some(doc) = doc else { return };
        if !self.seen_docs.insert(doc.start) {
            return;
        }
        let raw = doc.text(self.src);
        if !raw.contains('@') {
            return;
        }
        let base = doc.start as usize;
        let mut line_off = 0usize;
        for line in raw.split_inclusive('\n') {
            let here = line_off;
            line_off += line.len();
            let Some(at) = line.find('@') else { continue };
            let tag_end = line[at + 1..]
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
                .map(|i| at + 1 + i)
                .unwrap_or(line.len());
            let tag = line[at + 1..tag_end].to_ascii_lowercase();
            let tag = tag.trim_start_matches("phpstan-").trim_start_matches("psalm-");
            let body_raw = line[tag_end..].trim_end_matches(['\n', '\r']).trim_end_matches("*/");
            let lead = body_raw.len() - body_raw.trim_start().len();
            let body = body_raw.trim_start();
            let body_off = base + here + tag_end + lead;
            let (seg, seg_off, method) = match tag {
                "var"
                | "param"
                | "return"
                | "throws"
                | "property"
                | "property-read"
                | "property-write"
                | "mixin"
                | "extends"
                | "implements"
                | "use"
                | "template-extends"
                | "template-implements"
                | "template-use"
                | "see"
                | "param-out"
                | "self-out"
                | "this-out"
                | "assert"
                | "assert-if-true"
                | "assert-if-false" => {
                    let (ty, _) = docblock::split_type(body);
                    (ty, body_off, false)
                }
                "template" | "template-covariant" | "template-contravariant" => match body.find(" of ") {
                    Some(i) => {
                        let rest = &body[i + 4..];
                        let l = rest.len() - rest.trim_start().len();
                        (docblock::split_type(rest.trim_start()).0, body_off + i + 4 + l, false)
                    }
                    None => continue,
                },
                "method" => {
                    let end = body.rfind(')').map(|i| i + 1).unwrap_or(body.len());
                    (&body[..end], body_off, true)
                }
                _ => continue,
            };
            self.doc_names(seg, seg_off, method);
        }
    }

    /// Record the class names in one tag's type text `seg`, which starts at byte `off` of the file.
    fn doc_names(&mut self, seg: &str, off: usize, method: bool) {
        let b = seg.as_bytes();
        let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'\\' || c >= 0x80;
        let mut i = 0;
        while i < b.len() {
            if !word(b[i]) {
                i += 1;
                continue;
            }
            let start = i;
            while i < b.len() && word(b[i]) {
                i += 1;
            }
            let name = &seg[start..i];
            let prev = if start > 0 { b[start - 1] } else { b' ' };
            let next = b.get(i).copied().unwrap_or(b' ');
            let first = name.as_bytes()[0];
            if first.is_ascii_digit()
                || matches!(prev, b'$' | b'-' | b'\'' | b'"' | b':' | b'.')
                || next == b'-'
                || (method && next == b'(')
                || is_builtin_type(name)
                || types::is_non_class(name)
                || matches!(name.to_ascii_lowercase().as_str(), "this" | "static" | "self" | "parent")
            {
                continue;
            }
            // array-shape keys: `array{name: Foo}`
            if seg[i..].trim_start().starts_with(':') && !seg[i..].trim_start().starts_with("::") {
                continue;
            }
            let fqn = self.ctx.resolve_class(name);
            if self.index.find_class(&fqn).is_some() {
                let span = Span::new(off + start, off + i);
                self.out.refs.push(Reference { span, target: RefTarget::Class(fqn) });
            }
        }
    }

    /// `/** @var User $u */` before a statement: `$u` is a `User` (also without a variable name,
    /// for the variable assigned by the statement).
    pub(super) fn handle_doc_vars(&mut self, s: &Stmt) {
        let Some(doc) = s.doc else { return };
        let raw = doc.text(self.src);
        if !raw.contains("@var") {
            return;
        }
        let d = docblock::parse(raw);
        for tag in d.tags("var") {
            let (ty, name) = docblock::parse_typed_var(&tag.body);
            if ty.is_empty() {
                continue;
            }
            let classes = self.doc_classes(&ty);
            match name {
                Some(n) => {
                    // the `$name` inside the comment is where the variable is first seen
                    let at =
                        doc_var_span(raw, &n).map(|(a, b)| Span::new(doc.start as usize + a, doc.start as usize + b));
                    let decl = self.declare(&n, at.unwrap_or(s.span), classes.clone());
                    if let Some(at) = at {
                        self.out.refs.push(Reference {
                            span: at,
                            target: RefTarget::Variable { name: n.clone(), decl: Some(decl), classes: classes.clone() },
                        });
                    }
                    self.doc_vars.push((n, classes));
                }
                None => self.pending_doc_type = Some(classes),
            }
        }
    }
}
