//! Doc-comment type expressions encoded as flat "atoms" (strings), plus template binding.
//!
//! The type of an expression is a list of atoms ([`Types`]); more than one atom means a union.
//! Atom grammar:
//!   `App\User`              class (resolved FQN, no leading backslash)
//!   `Coll<int,App\User>`    generic class; args are unions joined by `|`, scalars become `mixed`
//!   `App\User[]`            array / list / iterable of the atom
//!   `@T`                    template parameter (unresolved)
//!   `class-string<App\U>`   `Foo::class` and `class-string<T>`
//!   `Closure<ret>`          closure / callable with return type
//!   `static`                late static binding
//! Scalars and `null` are not represented.
//!
//! Examples:
//!   `/** @var User|Admin $x */`          -> `["App\User", "App\Admin"]`
//!   `/** @var Collection<int, User> */`  -> `["Illuminate\Support\Collection<mixed,App\User>"]`
//!   `/** @var User[] */`                 -> `["App\User[]"]`
//!   `/** @var int */`                    -> `[]`

use rustc_hash::FxHashMap;

/// Possible types of a value, e.g. `["App\User", "App\Admin"]` for `User|Admin`. Empty when unknown.
pub type Types = Vec<String>;

/// Types of each parameter of a closure, by position: `[[User], [int-key types…]]`.
pub type ParamTypes = Vec<Types>;

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
    "interface-string",
    "trait-string",
    "enum-string",
    "non-negative-int",
    "non-positive-int",
    "non-zero-int",
    "uppercase-string",
    "non-empty-lowercase-string",
    "non-empty-uppercase-string",
    "associative-array",
    "non-empty-literal-string",
    "callable-array",
    "callable-object",
    "stringable-object",
    "int-range",
    "decimal-int-string",
    "html-escaped-string",
    "__stringandstringable",
];

pub fn is_non_class(name: &str) -> bool {
    let l = name.trim_start_matches('\\').to_ascii_lowercase();
    NON_CLASS.contains(&l.as_str())
}

struct Parser<'a> {
    c: Vec<char>,
    i: usize,
    resolve: &'a dyn Fn(&str) -> Option<String>,
}

/// Parse a doc type expression into atoms. `resolve` maps a written class name (or `self`, `static`,
/// `parent`, template names) to its encoded atom; `None` drops it.
pub fn parse_doc_type(ty: &str, resolve: &dyn Fn(&str) -> Option<String>) -> Types {
    let mut p = Parser { c: ty.chars().collect(), i: 0, resolve };
    let mut out = p.union();
    out.dedup();
    let mut seen = Vec::new();
    out.retain(|a| {
        let k = a.to_ascii_lowercase();
        if seen.contains(&k) {
            false
        } else {
            seen.push(k);
            true
        }
    });
    out
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<char> {
        self.c.get(self.i).copied()
    }

    fn skip_spaces(&mut self) {
        while self.peek().map(|c| c.is_whitespace()).unwrap_or(false) {
            self.i += 1;
        }
    }

    fn eat(&mut self, ch: char) -> bool {
        self.skip_spaces();
        if self.peek() == Some(ch) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn union(&mut self) -> Types {
        let mut out = Vec::new();
        loop {
            out.extend(self.atom());
            self.skip_spaces();
            match self.peek() {
                Some('|') | Some('&') => self.i += 1,
                _ => break,
            }
        }
        out
    }

    /// Skip a balanced `open..close` group starting at the current char.
    fn skip_group(&mut self, open: char, close: char) {
        let mut depth = 0;
        while let Some(c) = self.peek() {
            self.i += 1;
            if c == open {
                depth += 1;
            } else if c == close {
                depth -= 1;
                if depth == 0 {
                    return;
                }
            }
        }
    }

    fn skip_word(&mut self, w: &str) {
        self.skip_spaces();
        let end = self.i + w.len();
        if end <= self.c.len() && self.c[self.i..end].iter().collect::<String>() == w {
            self.i = end;
        }
    }

    /// One member of a union: `?Foo`, `(A|B)`, `Foo<X>[]`, `callable(): R`, `'literal'` …
    fn atom(&mut self) -> Types {
        self.skip_spaces();
        match self.peek() {
            None => Vec::new(),
            Some('?') => {
                self.i += 1;
                self.atom()
            }
            Some('(') => {
                let atoms = self.paren_group();
                self.with_array_suffixes(atoms)
            }
            Some(quote @ ('\'' | '"')) => {
                // string literal type: not a class
                self.i += 1;
                while let Some(c) = self.peek() {
                    self.i += 1;
                    if c == quote {
                        break;
                    }
                }
                self.with_array_suffixes(Vec::new())
            }
            Some(_) => self.named_atom(),
        }
    }

    /// `(A|B)` or the conditional type `($param is Cond ? Then : Else)` -> Then | Else.
    fn paren_group(&mut self) -> Types {
        self.i += 1;
        self.skip_spaces();
        if self.peek() != Some('$') {
            let atoms = self.union();
            self.eat(')');
            return atoms;
        }
        while self.peek().map(|c| !c.is_whitespace()).unwrap_or(false) {
            self.i += 1;
        }
        self.skip_spaces();
        self.skip_word("is");
        self.skip_word("not");
        self.atom();
        self.eat('?');
        let mut atoms = self.union();
        self.eat(':');
        atoms.extend(self.union());
        self.eat(')');
        atoms
    }

    /// `Foo`, `Foo<A, B>`, `array{...}`, `callable(...): R`, `Foo::BAR`, each with `[]` suffixes.
    fn named_atom(&mut self) -> Types {
        let start = self.i;
        while let Some(c) = self.peek() {
            if c.is_whitespace()
                || matches!(c, '|' | '&' | '<' | '>' | '[' | ']' | '(' | ')' | ',' | '{' | '}' | ':' | '=')
            {
                break;
            }
            self.i += 1;
        }
        let name: String = self.c[start..self.i].iter().collect();
        if name.is_empty() {
            // unexpected punctuation: consume one char so we always make progress
            self.i += 1;
            return Vec::new();
        }
        if self.peek() == Some(':') && self.c.get(self.i + 1) == Some(&':') {
            // `Foo::BAR` / `Foo::*` constant references
            self.i += 2;
            while let Some(c) = self.peek() {
                if c.is_whitespace() || matches!(c, '|' | '&' | ')' | ',' | '>') {
                    break;
                }
                self.i += 1;
            }
            return Vec::new();
        }
        let mut args: Vec<Types> = Vec::new();
        let lname = name.trim_start_matches('\\').to_ascii_lowercase();
        if self.peek() == Some('<') {
            self.i += 1;
            loop {
                args.push(self.union());
                if !self.eat(',') {
                    break;
                }
            }
            self.eat('>');
        } else if self.peek() == Some('{') {
            self.skip_group('{', '}');
        } else if self.peek() == Some('(')
            && matches!(lname.as_str(), "callable" | "closure" | "pure-callable" | "pure-closure")
        {
            return self.callable_type();
        }
        let atoms = self.build(&name, &lname, args);
        self.with_array_suffixes(atoms)
    }

    /// `callable(int): User` -> `Closure<User>` (the parameter list is skipped).
    fn callable_type(&mut self) -> Types {
        self.skip_group('(', ')');
        self.skip_spaces();
        let mut ret = Vec::new();
        if self.peek() == Some(':') {
            self.i += 1;
            ret = self.atom();
        }
        let ret_enc = ret.join("|");
        self.suffixes();
        vec![if ret_enc.is_empty() { "Closure".to_string() } else { format!("Closure<{ret_enc}>") }]
    }

    /// Wrap the atoms in one `[]` per `[]` suffix that follows.
    fn with_array_suffixes(&mut self, mut atoms: Types) -> Types {
        let n = self.suffixes();
        for _ in 0..n {
            atoms = atoms.into_iter().map(|a| format!("{a}[]")).collect();
        }
        atoms
    }

    /// Consume `[]` pairs (and skip `['key']` offset accesses). Returns the number of `[]`.
    fn suffixes(&mut self) -> usize {
        let mut n = 0;
        loop {
            self.skip_spaces();
            if self.peek() == Some('[') {
                if self.c.get(self.i + 1) == Some(&']') {
                    self.i += 2;
                    n += 1;
                } else {
                    self.skip_group('[', ']');
                }
            } else {
                return n;
            }
        }
    }

    fn build(&self, name: &str, lname: &str, args: Vec<Types>) -> Types {
        match lname {
            "array" | "non-empty-array" | "iterable" | "list" | "non-empty-list" | "associative-array" => {
                return match args.last() {
                    Some(v) => v.iter().map(|a| format!("{a}[]")).collect(),
                    None => Vec::new(),
                };
            }
            "class-string" | "interface-string" | "trait-string" | "enum-string" => {
                return match args.first() {
                    Some(v) if !v.is_empty() => vec![format!("class-string<{}>", v.join("|"))],
                    _ => Vec::new(),
                };
            }
            "$this" => return vec!["static".to_string()],
            "self" | "static" | "parent" => return (self.resolve)(lname).into_iter().collect(),
            _ => {}
        }
        if NON_CLASS.contains(&lname) {
            return Vec::new();
        }
        if name.contains(['-', '$', '*', '\'', '"', '.'])
            || name.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(true)
        {
            return Vec::new();
        }
        let Some(r) = (self.resolve)(name) else { return Vec::new() };
        if args.is_empty() || r.starts_with('@') {
            return vec![r];
        }
        if args.iter().all(|a| a.is_empty()) {
            return vec![r];
        }
        let enc: Vec<String> =
            args.iter().map(|a| if a.is_empty() { "mixed".to_string() } else { a.join("|") }).collect();
        vec![format!("{r}<{}>", enc.join(","))]
    }
}

// ---- atom helpers -------------------------------------------------------------------------

/// `Foo<A,B>` -> `["A", "B"]`; empty for non-generic atoms and arrays.
pub fn generic_args(atom: &str) -> Vec<&str> {
    if atom.ends_with("[]") {
        return Vec::new();
    }
    let Some(i) = atom.find('<') else { return Vec::new() };
    let Some(j) = atom.rfind('>') else { return Vec::new() };
    if j <= i + 1 {
        return Vec::new();
    }
    split_top(&atom[i + 1..j], ',')
}

/// Split at `sep` outside of `<>`.
pub fn split_top(s: &str, sep: char) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '<' | '(' | '{' => depth += 1,
            '>' | ')' | '}' => depth -= 1,
            c if c == sep && depth == 0 => {
                out.push(&s[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

/// Atoms of a generic argument (a `|`-joined union).
pub fn arg_atoms(arg: &str) -> Types {
    if arg == "mixed" || arg.is_empty() {
        return Vec::new();
    }
    split_top(arg, '|').into_iter().map(|s| s.to_string()).collect()
}

/// `X[]` -> `X`
pub fn array_elem(atom: &str) -> Option<&str> {
    atom.strip_suffix("[]")
}

/// Class part of an atom (`Foo<A>` -> `Foo`). Arrays are returned unchanged so they never match a class.
pub fn atom_base(atom: &str) -> &str {
    if atom.ends_with("[]") {
        return atom;
    }
    match atom.find('<') {
        Some(i) => &atom[..i],
        None => atom,
    }
}

pub fn has_template(atom: &str) -> bool {
    atom.contains('@')
}

#[derive(Default, Debug, Clone)]
pub struct Bindings(pub FxHashMap<String, Types>);

impl Bindings {
    pub fn bind(&mut self, name: &str, atoms: impl IntoIterator<Item = String>) {
        let e = self.0.entry(name.to_string()).or_default();
        for a in atoms {
            if !e.iter().any(|x| x.eq_ignore_ascii_case(&a)) {
                e.push(a);
            }
        }
    }

    pub fn get(&self, name: &str) -> Option<&Types> {
        self.0.get(name)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Substitute template parameters in `atoms`. Unbound parameters vanish.
pub fn subst(atoms: &[impl AsRef<str>], b: &Bindings) -> Types {
    let mut out: Types = Vec::new();
    for a in atoms {
        for s in subst_atom(a.as_ref(), b) {
            if !out.iter().any(|x| x.eq_ignore_ascii_case(&s)) {
                out.push(s);
            }
        }
    }
    out
}

fn subst_atom(atom: &str, b: &Bindings) -> Types {
    if !has_template(atom) {
        return vec![atom.to_string()];
    }
    if let Some(inner) = array_elem(atom) {
        return subst_atom(inner, b).into_iter().map(|e| format!("{e}[]")).collect();
    }
    if let Some(n) = atom.strip_prefix('@') {
        return b.get(n).cloned().unwrap_or_default();
    }
    if let Some(i) = atom.find('<') {
        let base = &atom[..i];
        let args: Vec<String> = generic_args(atom)
            .into_iter()
            .map(|a| {
                let atoms: Vec<String> = split_top(a, '|').into_iter().flat_map(|x| subst_atom(x, b)).collect();
                if atoms.is_empty() {
                    "mixed".to_string()
                } else {
                    atoms.join("|")
                }
            })
            .collect();
        if args.iter().all(|a| a == "mixed") {
            return vec![base.to_string()];
        }
        return vec![format!("{base}<{}>", args.join(","))];
    }
    vec![atom.to_string()]
}

/// Replace the late-static-binding atom `static` inside generic arguments (`Coll<int,static>`).
pub fn replace_static(atom: &str, with: &str) -> String {
    if atom == "static" {
        return with.to_string();
    }
    if let Some(inner) = array_elem(atom) {
        return format!("{}[]", replace_static(inner, with));
    }
    let Some(i) = atom.find('<') else { return atom.to_string() };
    if !atom.contains("static") {
        return atom.to_string();
    }
    let args: Vec<String> = generic_args(atom)
        .into_iter()
        .map(|a| split_top(a, '|').into_iter().map(|x| replace_static(x, with)).collect::<Vec<_>>().join("|"))
        .collect();
    format!("{}<{}>", &atom[..i], args.join(","))
}

/// Bind template parameters occurring in `pattern` by matching it against the `actual` atoms.
/// `iter_elem` yields the element atoms of a non-array iterable atom (e.g. a generic collection).
pub fn unify(pattern: &str, actual: &[String], b: &mut Bindings, iter_elem: &dyn Fn(&str) -> Types) {
    if !has_template(pattern) || actual.is_empty() {
        return;
    }
    if let Some(inner) = array_elem(pattern) {
        let mut elems = Vec::new();
        for a in actual {
            match array_elem(a) {
                Some(e) => elems.push(e.to_string()),
                None => elems.extend(iter_elem(a)),
            }
        }
        unify(inner, &elems, b, iter_elem);
        return;
    }
    if let Some(n) = pattern.strip_prefix('@') {
        b.bind(n, actual.iter().cloned());
        return;
    }
    // generic pattern `Foo<@A,@B>`: match positionally against actuals with the same class
    let pbase = atom_base(pattern);
    let pargs = generic_args(pattern);
    for a in actual {
        if !atom_base(a).eq_ignore_ascii_case(pbase) {
            continue;
        }
        let aargs = generic_args(a);
        for (p, x) in pargs.iter().zip(aargs.iter()) {
            for pp in split_top(p, '|') {
                unify(pp, &arg_atoms(x), b, iter_elem);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn res(n: &str) -> Option<String> {
        match n {
            "T" | "TKey" | "TValue" => Some(format!("@{n}")),
            "static" => Some("static".into()),
            _ => Some(format!("N\\{}", n.trim_start_matches('\\'))),
        }
    }

    fn p(s: &str) -> Vec<String> {
        parse_doc_type(s, &res)
    }

    #[test]
    fn parses() {
        assert_eq!(p("User[]"), ["N\\User[]"]);
        assert_eq!(p("?User"), ["N\\User"]);
        assert_eq!(p("User|null|int"), ["N\\User"]);
        assert_eq!(p("array<int, User>"), ["N\\User[]"]);
        assert_eq!(p("list<User|Team>"), ["N\\User[]", "N\\Team[]"]);
        assert_eq!(p("(User|Team)[]"), ["N\\User[]", "N\\Team[]"]);
        assert_eq!(p("User[][]"), ["N\\User[][]"]);
        assert_eq!(p("Collection<int, User>"), ["N\\Collection<int,N\\User>".replace("int", "mixed")]);
        assert_eq!(p("Collection<TKey, TValue>"), ["N\\Collection<@TKey,@TValue>"]);
        assert_eq!(p("T"), ["@T"]);
        assert_eq!(p("T[]"), ["@T[]"]);
        assert_eq!(p("class-string<T>"), ["class-string<@T>"]);
        assert_eq!(p("callable(int): T"), ["Closure<@T>"]);
        assert_eq!(p("Closure(A, B): User|null"), ["Closure<N\\User>"]);
        assert_eq!(p("array{a: int, b: User}"), Vec::<String>::new());
        assert_eq!(p("$this"), ["static"]);
        assert_eq!(p("Foo::BAR|User"), ["N\\User"]);
        assert_eq!(p("int-mask<1,2>|User"), ["N\\User"]);
        assert_eq!(p("array<string, array<int, User>>"), ["N\\User[][]"]);
    }

    #[test]
    fn binds() {
        let mut b = Bindings::default();
        let no = |_: &str| Vec::<String>::new();
        unify("@T[]", &["N\\User[]".to_string()], &mut b, &no);
        assert_eq!(b.get("T").unwrap(), &["N\\User".to_string()]);
        let mut b = Bindings::default();
        unify("N\\Coll<@K,@V>", &["N\\Coll<mixed,N\\User>".to_string()], &mut b, &no);
        assert_eq!(b.get("V").unwrap(), &["N\\User".to_string()]);
        assert!(b.get("K").is_none());
        let mut b = Bindings::default();
        unify("class-string<@T>", &["class-string<N\\Foo>".to_string()], &mut b, &no);
        assert_eq!(
            subst(&["@T", "N\\Coll<mixed,@T>", "@T[]", "@U"], &b),
            ["N\\Foo", "N\\Coll<mixed,N\\Foo>", "N\\Foo[]"]
        );
    }
}
