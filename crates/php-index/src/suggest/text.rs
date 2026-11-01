//! Type text (signature hints and docblock types) and literal values -> [`Atom`]s.

use super::{Atom, Partial};
use crate::types;

/// Atoms of a type written as text: `?int`, `User|false`, `array<string, User>`, `static`.
///
/// `classes` are the resolved class atoms of the same declaration (`Symbol::type_classes`,
/// `ParamInfo::classes`); a class word is looked up there by its short name. `container` is the
/// class the declaration belongs to (`self`). `static` / `$this` set `need_classes`.
/// None when part of the type has no PHP equivalent (`resource`, `key-of<…>`, an unknown name) or
/// is a template (`@template T`): the analysis drops template parameters it cannot bind, so the
/// classes it reports for them may be incomplete.
pub(super) fn type_text(text: &str, classes: &[impl AsRef<str>], container: Option<&str>) -> Option<Partial> {
    let mut out = Partial::default();
    let text = text.trim();
    let text = match text.strip_prefix('?') {
        Some(rest) => {
            out.ty.add(Atom::Null);
            rest
        }
        None => text,
    };
    for part in types::split_top(text, '|') {
        let part = part.trim();
        if part.is_empty() {
            return None;
        }
        if let Some(inner) = part.strip_prefix('(').and_then(|p| p.strip_suffix(')')) {
            out.merge(type_text(inner, classes, container)?);
            continue;
        }
        if part.contains('&') {
            return None; // intersection types are not suggested
        }
        if let Some(elem) = part.strip_suffix("[]") {
            out.ty.add(Atom::Array);
            if let Some(Atom::Class(c)) = class_word(elem, classes) {
                out.ty.add(Atom::ArrayOf(c));
            }
            continue;
        }
        let base = part.split(['<', '{', '(']).next().unwrap_or(part).trim();
        if let Some(atoms) = keyword(base) {
            if atoms.is_empty() {
                return None;
            }
            for a in atoms {
                match a {
                    Atom::Static => out.need_classes = true,
                    a => out.ty.add(a),
                }
            }
            continue;
        }
        match base.to_ascii_lowercase().as_str() {
            "self" => out.ty.add(Atom::Class(container?.to_string())),
            _ => match class_word(base, classes)? {
                Atom::Static => return None,
                a => out.ty.add(a),
            },
        }
    }
    Some(out)
}

/// A class name of a type text, resolved through `classes`. `Static` stands for a template.
fn class_word(word: &str, classes: &[impl AsRef<str>]) -> Option<Atom> {
    let word = word.trim().trim_start_matches('\\');
    if word.is_empty() || !word.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '\\' || !c.is_ascii()) {
        return None;
    }
    let short = word.rsplit('\\').next().unwrap_or(word);
    for c in classes {
        let c = c.as_ref();
        let base = types::atom_base(c);
        if let Some(t) = base.strip_prefix('@') {
            if t.eq_ignore_ascii_case(word) {
                return Some(Atom::Static);
            }
            continue;
        }
        if base.rsplit('\\').next().is_some_and(|s| s.eq_ignore_ascii_case(short)) {
            return Some(Atom::Class(base.trim_start_matches('\\').to_string()));
        }
    }
    None
}

/// Built-in type names (`int`, `non-empty-string`, `list`, `$this`, …); None for class names,
/// an empty list for built-in names without a PHP equivalent.
fn keyword(base: &str) -> Option<Vec<Atom>> {
    use Atom::*;
    let lower = base.to_ascii_lowercase();
    let atoms = match lower.as_str() {
        "int" | "integer" | "positive-int" | "negative-int" | "non-negative-int" | "non-positive-int"
        | "non-zero-int" | "int-mask" | "int-mask-of" | "literal-int" => vec![Int],
        "float" | "double" => vec![Float],
        "string"
        | "non-empty-string"
        | "numeric-string"
        | "literal-string"
        | "callable-string"
        | "lowercase-string"
        | "uppercase-string"
        | "non-falsy-string"
        | "truthy-string"
        | "class-string"
        | "interface-string"
        | "trait-string"
        | "enum-string"
        | "non-empty-lowercase-string" => vec![String],
        "bool" | "boolean" => vec![Bool],
        "true" => vec![True],
        "false" => vec![False],
        "null" => vec![Null],
        "void" => vec![Void],
        "never" | "never-return" | "never-returns" | "no-return" | "noreturn" => vec![Never],
        "mixed" => vec![Mixed],
        "object" => vec![Object],
        "callable" | "pure-callable" => vec![Callable],
        "iterable" => vec![Iterable],
        "array" | "list" | "non-empty-array" | "non-empty-list" | "associative-array" => vec![Array],
        "numeric" | "number" => vec![Int, Float],
        "scalar" => vec![Int, Float, String, Bool],
        "array-key" => vec![Int, String],
        "static" | "$this" => vec![Static],
        _ => match literal(base) {
            Some(a) => vec![a],
            // `resource`, `key-of`, `value-of`, … exist but have no PHP equivalent
            None if lower.contains('-') || lower.ends_with("resource") => Vec::new(),
            None => return None,
        },
    };
    Some(atoms)
}

/// Type of a literal as written: `'a'`, `"b"`, `<<<EOT`, `12`, `0x1F`, `1.5`, `1e3`.
pub(super) fn literal(text: &str) -> Option<Atom> {
    let t = text.trim();
    let t = t.strip_prefix(['b', 'B']).filter(|r| r.starts_with(['\'', '"'])).unwrap_or(t);
    if t.starts_with(['\'', '"', '`']) || t.starts_with("<<<") {
        return Some(Atom::String);
    }
    let t = t.strip_prefix('-').unwrap_or(t);
    if !t.starts_with(|c: char| c.is_ascii_digit() || c == '.') {
        return None;
    }
    let lower = t.to_ascii_lowercase();
    if lower.starts_with("0x") || lower.starts_with("0b") || lower.starts_with("0o") {
        return Some(Atom::Int);
    }
    if !lower.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | '_' | '+' | '-')) {
        return None;
    }
    Some(if lower.contains(['.', 'e']) { Atom::Float } else { Atom::Int })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::suggest::SuggestedType;

    fn ty(text: &str, classes: &[&str]) -> Option<(SuggestedType, bool)> {
        type_text(text, classes, Some("App\\Me")).map(|p| (p.ty, p.need_classes))
    }

    #[test]
    fn type_texts() {
        use Atom::*;
        assert_eq!(ty("?int", &[]), Some((SuggestedType::of([Int, Null]), false)));
        assert_eq!(
            ty("User|false", &["App\\User"]),
            Some((SuggestedType::of([Class("App\\User".into()), False]), false))
        );
        assert_eq!(ty("array<string, int>", &[]), Some((SuggestedType::of([Array]), false)));
        assert_eq!(
            ty("User[]", &["App\\User[]"]),
            Some((SuggestedType::of([Array]), false)),
            "an array atom is not a class"
        );
        assert_eq!(
            ty("User[]", &["App\\User"]),
            Some((SuggestedType::of([Array, ArrayOf("App\\User".into())]), false))
        );
        assert_eq!(ty("self", &[]), Some((SuggestedType::of([Class("App\\Me".into())]), false)));
        assert_eq!(ty("static", &[]), Some((SuggestedType::default(), true)));
        assert_eq!(ty("T", &["@T"]), None, "templates are not trusted");
        assert_eq!(ty("resource", &[]), None);
        assert_eq!(ty("key-of<X>", &[]), None);
        assert_eq!(ty("Unknown", &[]), None);
        assert_eq!(ty("non-empty-string|null", &[]), Some((SuggestedType::of([String, Null]), false)));
        assert_eq!(
            ty("Collection<int, User>", &["Illuminate\\Support\\Collection<mixed,App\\User>"]).map(|t| t.0),
            Some(SuggestedType::of([Class("Illuminate\\Support\\Collection".into())]))
        );
    }

    #[test]
    fn literals() {
        assert_eq!(literal("'a'"), Some(Atom::String));
        assert_eq!(literal("<<<EOT\nx\nEOT"), Some(Atom::String));
        assert_eq!(literal("12"), Some(Atom::Int));
        assert_eq!(literal("0x1F"), Some(Atom::Int));
        assert_eq!(literal("1.5"), Some(Atom::Float));
        assert_eq!(literal("1_000"), Some(Atom::Int));
        assert_eq!(literal("1e3"), Some(Atom::Float));
        assert_eq!(literal("FOO"), None);
    }
}
