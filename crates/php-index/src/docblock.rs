//! Minimal PHPDoc parser.

#[derive(Debug, Clone, Default)]
pub struct DocTag {
    pub name: String,
    pub body: String,
}

#[derive(Debug, Clone, Default)]
pub struct DocBlock {
    /// Description text without tags.
    pub text: String,
    pub tags: Vec<DocTag>,
}

impl DocBlock {
    pub fn tags<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a DocTag> + 'a {
        self.tags.iter().filter(move |t| t.name == name)
    }

    pub fn tag(&self, name: &str) -> Option<&DocTag> {
        self.tags.iter().find(|t| t.name == name)
    }

    /// Like `tag`, but `@phpstan-<name>` / `@psalm-<name>` (more precise) win over `@<name>`.
    pub fn tag_pref(&self, name: &str) -> Option<&DocTag> {
        self.tags_pref(name).into_iter().next()
    }

    /// All `@name` tags, with the `phpstan-`/`psalm-` variants first.
    pub fn tags_pref(&self, name: &str) -> Vec<&DocTag> {
        let ps = format!("phpstan-{name}");
        let pm = format!("psalm-{name}");
        let mut out: Vec<&DocTag> = self.tags.iter().filter(|t| t.name == ps || t.name == pm).collect();
        out.extend(self.tags.iter().filter(|t| t.name == name));
        out
    }

    /// `@template T [of Bound]` -> (name, bound type text).
    pub fn templates(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for t in &self.tags {
            let n = t.name.strip_prefix("phpstan-").or_else(|| t.name.strip_prefix("psalm-")).unwrap_or(&t.name);
            if !matches!(n, "template" | "template-covariant" | "template-contravariant") {
                continue;
            }
            let mut it = t.body.split_whitespace();
            let Some(name) = it.next() else { continue };
            let rest = t.body[name.len()..].trim_start();
            let bound = match rest.split_whitespace().next() {
                Some("of") | Some("as") => split_type(rest[2..].trim_start()).0.to_string(),
                _ => String::new(),
            };
            out.push((name.to_string(), bound));
        }
        out
    }

    /// Type texts of `@extends X<..>`, `@implements X<..>`, `@use X<..>`.
    pub fn inherits(&self) -> Vec<String> {
        let mut out = Vec::new();
        for t in &self.tags {
            let n = t.name.strip_prefix("phpstan-").or_else(|| t.name.strip_prefix("psalm-")).unwrap_or(&t.name);
            let n = n.strip_prefix("template-").unwrap_or(n);
            if matches!(n, "extends" | "implements" | "use") {
                let ty = split_type(&t.body).0;
                if !ty.is_empty() {
                    out.push(ty.to_string());
                }
            }
        }
        out
    }

    pub fn is_deprecated(&self) -> bool {
        self.tag("deprecated").is_some()
    }
}

pub fn parse(raw: &str) -> DocBlock {
    let body = raw.trim();
    let body = body.strip_prefix("/**").unwrap_or(body);
    let body = body.strip_suffix("*/").unwrap_or(body);
    let mut text = String::new();
    let mut tags: Vec<DocTag> = Vec::new();
    for line in body.lines() {
        let l = line.trim();
        let l = l.strip_prefix('*').unwrap_or(l);
        let l = l.strip_prefix(' ').unwrap_or(l);
        let lt = l.trim_end();
        if let Some(rest) = lt.trim_start().strip_prefix('@') {
            let (name, b) = match rest.find(|c: char| c.is_whitespace()) {
                Some(i) => (&rest[..i], rest[i..].trim()),
                None => (rest, ""),
            };
            tags.push(DocTag { name: name.to_ascii_lowercase(), body: b.to_string() });
        } else if let Some(last) = tags.last_mut() {
            if !lt.is_empty() {
                last.body.push(' ');
                last.body.push_str(lt.trim());
            }
        } else {
            text.push_str(lt);
            text.push('\n');
        }
    }
    DocBlock { text: text.trim().to_string(), tags }
}

/// Split `Type rest` where the type may contain spaces inside `<>`, `{}`, `()`.
pub fn split_type(body: &str) -> (&str, &str) {
    let mut depth = 0i32;
    let bytes: Vec<(usize, char)> = body.char_indices().collect();
    for (k, &(i, c)) in bytes.iter().enumerate() {
        match c {
            '<' | '{' | '(' | '[' => depth += 1,
            '>' | '}' | ')' | ']' => depth -= 1,
            c if c.is_whitespace() && depth <= 0 => {
                // `callable(A): B`, `A | B`: a joiner on either side keeps the type going
                let prev = bytes[..k].iter().rev().map(|x| x.1).find(|c| !c.is_whitespace());
                let next = bytes[k..].iter().map(|x| x.1).find(|c| !c.is_whitespace());
                let joiner = |c: Option<char>| matches!(c, Some(':') | Some('|') | Some('&'));
                if i > 0 && (joiner(prev) || joiner(next)) {
                    continue;
                }
                return (&body[..i], body[i..].trim_start());
            }
            _ => {}
        }
    }
    (body, "")
}

const NON_CLASS: &[&str] = &[
    "int",
    "integer",
    "string",
    "bool",
    "boolean",
    "float",
    "double",
    "array",
    "object",
    "mixed",
    "void",
    "null",
    "callable",
    "iterable",
    "resource",
    "false",
    "true",
    "never",
    "numeric",
    "scalar",
    "list",
    "class-string",
    "callable-string",
    "number",
    "self",
    "static",
    "parent",
    "key-of",
    "value-of",
    "positive-int",
    "negative-int",
    "non-empty-string",
    "non-empty-array",
    "array-key",
    "literal-string",
    "truthy-string",
    "numeric-string",
    "lowercase-string",
    "non-falsy-string",
    "int-mask",
    "int-mask-of",
    "closed-resource",
    "open-resource",
    "empty",
    "pure-callable",
    "pure-closure",
    "never-return",
    "no-return",
    "never-returns",
    "non-empty-list",
];

/// Top-level class-like names mentioned by a doc type (`Foo|null`, `?Foo`, `Collection<int, Foo>`).
/// Array-ish members like `Foo[]` are skipped. `$this` is reported as `static`.
pub fn class_names(ty: &str) -> Vec<String> {
    let mut flat = String::new();
    let mut depth = 0i32;
    for c in ty.chars() {
        match c {
            '<' | '{' | '(' => depth += 1,
            '>' | '}' | ')' => depth -= 1,
            _ if depth == 0 => flat.push(c),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for part in flat.split(['|', '&']) {
        let p = part.trim().trim_start_matches('?');
        if p.is_empty() || p.ends_with("[]") || p.contains('[') {
            continue;
        }
        if p == "$this" {
            out.push("static".to_string());
            continue;
        }
        let lower = p.to_ascii_lowercase();
        if NON_CLASS.contains(&lower.as_str()) && !matches!(lower.as_str(), "self" | "static" | "parent") {
            continue;
        }
        if p.contains('-') || p.contains('$') || p.contains('*') || p.contains('\'') || p.contains('"') {
            continue;
        }
        if p.bytes().next().map(|b| b.is_ascii_digit()).unwrap_or(true) {
            continue;
        }
        out.push(p.to_string());
    }
    out
}

/// `Type $name description` -> (type, name)
pub fn parse_typed_var(body: &str) -> (String, Option<String>) {
    let (ty, rest) = split_type(body);
    if ty.starts_with('$') {
        return (String::new(), Some(ty.trim_start_matches('$').to_string()));
    }
    let name = rest
        .split_whitespace()
        .next()
        .filter(|w| w.starts_with('$') || w.starts_with("&$") || w.starts_with("...$"))
        .map(|w| w.trim_start_matches(['&', '.', '$']).to_string());
    (ty.to_string(), name)
}

/// `@property` body: `Type $name`, `$name`, or the sloppy `Type name` (no `$`).
pub fn parse_property_tag(body: &str) -> (String, Option<String>) {
    let (ty, name) = parse_typed_var(body);
    if name.is_some() {
        return (ty, name);
    }
    let (ty, rest) = split_type(body);
    let word = rest.split_whitespace().next().unwrap_or("");
    if !ty.is_empty() && !word.is_empty() && word.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return (ty.to_string(), Some(word.to_string()));
    }
    (ty.to_string(), None)
}
