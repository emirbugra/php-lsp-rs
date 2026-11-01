//! `@property` and `@method` tags of a class docblock declare members that are not in the code.

use super::collect::SymbolCollector;
use super::{Symbol, SymbolKind};
use crate::docblock::{self, DocBlock, DocTag};
use crate::istr::{istrs, IStr};
use php_syntax::*;

/// Where the members of one class docblock are declared.
pub(super) struct DocClass<'c> {
    pub(super) fqn: &'c str,
    pub(super) parent: Option<&'c str>,
    /// The class declaration (range of every member declared in the docblock).
    pub(super) range: Span,
    /// Fallback position when a name is not found on its tag line.
    pub(super) name_span: Span,
    pub(super) doc: Option<Span>,
}

impl SymbolCollector<'_> {
    pub(super) fn doc_members(&mut self, d: &DocBlock, class: &DocClass) {
        for tag in &d.tags {
            match tag.name.as_str() {
                "property" | "property-read" | "property-write" => self.doc_property(tag, class),
                "method" => self.doc_method(tag, class),
                _ => {}
            }
        }
    }

    /// `@property Type $name`
    fn doc_property(&mut self, tag: &DocTag, class: &DocClass) {
        let (ty, name) = docblock::parse_property_tag(&tag.body);
        let Some(name) = name else { return };
        let at = self.tag_name_span(class.doc, &tag.name, &name, class.name_span);
        let mut sym = Symbol::member(SymbolKind::Property, name.clone(), class.fqn, class.range, at);
        sym.is_virtual = true;
        sym.type_classes = istrs(self.doc_atoms(&ty, Some(class.fqn), class.parent));
        sym.detail = IStr::from(format!("{}${}", if ty.is_empty() { String::new() } else { format!("{ty} ") }, name));
        if !ty.is_empty() {
            sym.ty = Some(IStr::from(ty));
        }
        self.out.push(sym);
    }

    /// `@method [static] Type name(args)`
    fn doc_method(&mut self, tag: &DocTag, class: &DocClass) {
        let mut body = tag.body.trim();
        let mut is_static = false;
        if let Some(rest) = body.strip_prefix("static ") {
            is_static = true;
            body = rest.trim_start();
        }
        // `@method name` without a parameter list is sloppy but common
        let paren = body.find('(').unwrap_or(body.len());
        let head = body[..paren].trim();
        let mut parts = head.split_whitespace().collect::<Vec<_>>();
        let Some(name) = parts.pop() else { return };
        let ty = parts.join(" ");
        let at = self.tag_name_span(class.doc, "method", name, class.name_span);
        let mut sym = Symbol::member(SymbolKind::Method, name.to_string(), class.fqn, class.range, at);
        sym.is_virtual = true;
        sym.is_static = is_static;
        sym.type_classes = istrs(self.doc_atoms(&ty, Some(class.fqn), class.parent));
        let close = body.rfind(')').unwrap_or(body.len().saturating_sub(1));
        let args = if paren < body.len() { body.get(paren + 1..close).unwrap_or("") } else { "" };
        sym.detail = IStr::from(format!(
            "public{} function {}({}){}",
            if is_static { " static" } else { "" },
            name,
            args,
            if ty.is_empty() { String::new() } else { format!(": {ty}") }
        ));
        if !ty.is_empty() {
            sym.ty = Some(IStr::from(ty));
        }
        self.out.push(sym);
    }

    /// Span of `name` on the `@tag` line of the docblock, so go-to-definition lands on the tag.
    fn tag_name_span(&self, doc: Option<Span>, tag: &str, name: &str, fallback: Span) -> Span {
        let Some(doc) = doc else { return fallback };
        let Some(text) = self.src.get(doc.start as usize..doc.end as usize) else { return fallback };
        let marker = format!("@{tag}");
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        let mut off = 0usize;
        for line in text.split_inclusive('\n') {
            if let Some(m) = line.find(&marker) {
                let after = &line[m + marker.len()..];
                let mut from = 0;
                while let Some(i) = after[from..].find(name) {
                    let at = from + i;
                    let before_ok = !after[..at].chars().next_back().map(is_word).unwrap_or(false);
                    let after_ok = !after[at + name.len()..].chars().next().map(is_word).unwrap_or(false);
                    if before_ok && after_ok {
                        let start = doc.start as usize + off + m + marker.len() + at;
                        return Span { start: start as u32, end: (start + name.len()) as u32 };
                    }
                    from = at + name.len();
                }
            }
            off += line.len();
        }
        fallback
    }
}
