//! Laravel Blade templates as PHP code.
//!
//! [`to_php`] turns a template into PHP code of exactly the same byte length, with every line
//! break and every non-ASCII character where it was. Byte offsets, lines and UTF-16 columns of
//! the template and of the code are therefore the same, and every feature that works on PHP works
//! on the template without translating positions. The code is parsed with [`crate::parse_code`]
//! (it has no `<?php` tag).
//!
//! What becomes code (the rest turns into spaces):
//! - `{{ expr }}` / `{!! expr !!}` → `;  expr; ` (every construct starts with `;`, so it never
//!   joins the code before it);
//! - `@php … @endphp`, `@php(expr)`, `<?php … ?>`, `<?= … ?>`;
//! - directives with arguments: `@if(…)` → `;if(…)`, `@foreach(…)` → `;foreach(…)` (the loop
//!   variables get declared), `@forelse` → `;foreach`, `@elseif` / `@unless` → `if`, view
//!   directives `@extends` / `@include` / `@includeIf` / `@component` / `@each` → `view(…)`,
//!   `@lang` / `@choice` → `__(…)`, `@use('A\B')` → `use A\B;`, keyword-named directives
//!   (`@switch`, `@case`, `@class`) → a parenthesized expression, any other `@name(…)` → a call
//!   `name(…)`;
//! - component attributes `:name="expr"` inside `<x-…>` tags.
//!
//! Left out: `{{-- comments --}}`, `@{{ escaped }}`, `@verbatim … @endverbatim`, directives
//! without arguments (`@endif`, `@csrf`, `@else`).
//!
//! Non-ASCII text outside code is kept (replacing it would change byte or column counts); the
//! identifiers it lexes into are junk statements of their own, so diagnostics in templates are
//! limited to checks that cannot be fooled by them.

/// Does the path / URI name a Blade template?
pub fn is_blade(path: &str) -> bool {
    path.len() >= 10 && path[path.len() - 10..].eq_ignore_ascii_case(".blade.php")
}

/// Directives whose name is a PHP keyword: written as a parenthesized expression.
const KEYWORD_DIRECTIVES: &[&str] = &[
    "switch",
    "case",
    "class",
    "default",
    "break",
    "continue",
    "unset",
    "while",
    "for",
    "foreach",
    "if",
    "else",
    "elseif",
    "endif",
    "array",
    "list",
    "return",
    "echo",
    "print",
    "new",
    "function",
    "fn",
    "match",
    "include",
    "require",
    "use",
    "extends",
    "static",
    "global",
    "throw",
    "try",
    "catch",
    "finally",
    "do",
    "goto",
    "declare",
    "namespace",
    "trait",
    "interface",
    "enum",
    "abstract",
    "final",
    "public",
    "private",
    "protected",
    "const",
    "var",
    "exit",
    "die",
    "eval",
    "isset",
    "empty",
    "instanceof",
    "insteadof",
    "clone",
    "yield",
    "and",
    "or",
    "xor",
    "as",
    "callable",
];

struct Out {
    bytes: Vec<u8>,
}

impl Out {
    /// Copy `src[from..to]` as code.
    fn code(&mut self, src: &[u8], from: usize, to: usize) {
        self.bytes[from..to].copy_from_slice(&src[from..to]);
    }

    /// Write ASCII `text` at `at` (over blanks).
    fn put(&mut self, at: usize, text: &str) {
        self.bytes[at..at + text.len()].copy_from_slice(text.as_bytes());
    }
}

pub fn to_php(src: &str) -> String {
    let b = src.as_bytes();
    // blank every ASCII byte but line breaks; keep non-ASCII bytes (whole characters)
    let mut out =
        Out { bytes: b.iter().map(|&c| if c.is_ascii() && c != b'\n' && c != b'\r' { b' ' } else { c }).collect() };
    let n = b.len();
    let mut i = 0;
    let mut in_component = false;
    while i < n {
        let c = b[i];
        if starts(b, i, b"{{--") {
            i = find(b, i + 4, b"--}}").map(|j| j + 4).unwrap_or(n);
        } else if c == b'@' && starts(b, i + 1, b"{{") {
            i = find(b, i + 3, b"}}").map(|j| j + 2).unwrap_or(n);
        } else if starts(b, i, b"{!!") {
            let end = close_of(b, i + 3, b"!!}").unwrap_or(n);
            out.put(i, ";");
            out.code(b, i + 3, end);
            if end + 3 <= n {
                out.put(end, ";");
            }
            i = (end + 3).min(n);
        } else if starts(b, i, b"{{") {
            let end = close_of(b, i + 2, b"}}").unwrap_or(n);
            out.put(i, ";");
            out.code(b, i + 2, end);
            if end + 2 <= n {
                out.put(end, ";");
            }
            i = (end + 2).min(n);
        } else if starts(b, i, b"<?php") || starts(b, i, b"<?=") {
            let open = if b[i + 2] == b'=' { 3 } else { 5 };
            let end = find(b, i + open, b"?>").unwrap_or(n);
            out.put(i, ";");
            out.code(b, i + open, end);
            if end + 2 <= n {
                out.put(end, ";");
            }
            i = (end + 2).min(n);
        } else if c == b'@' && (i == 0 || !is_word(b[i - 1])) && b.get(i + 1).is_some_and(|&x| x.is_ascii_alphabetic())
        {
            i = directive(b, i, &mut out);
        } else if c == b'<' && (starts(b, i, b"<x-") || starts(b, i, b"<x:")) {
            in_component = true;
            i += 3;
        } else if in_component && c == b'>' {
            in_component = false;
            i += 1;
        } else if in_component
            && c == b':'
            && i > 0
            && b[i - 1].is_ascii_whitespace()
            && b.get(i + 1).is_some_and(|&x| x != b':')
        {
            i = component_attribute(b, i, &mut out);
        } else if in_component && (c == b'"' || c == b'\'') {
            // a plain attribute value: skip it, except for `{{ }}` inside
            let q = c;
            i += 1;
            while i < n && b[i] != q {
                if starts(b, i, b"{{") {
                    break;
                }
                i += 1;
            }
            if i < n && b[i] == q {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    // only whole ASCII bytes were replaced, so this is valid UTF-8
    String::from_utf8(out.bytes).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

/// `@name` at `at`; returns where scanning continues.
fn directive(b: &[u8], at: usize, out: &mut Out) -> usize {
    let n = b.len();
    let name_end = (at + 1..n).find(|&j| !is_word(b[j])).unwrap_or(n);
    let name = std::str::from_utf8(&b[at + 1..name_end]).unwrap_or("");
    match name {
        "verbatim" => return find(b, name_end, b"@endverbatim").map(|j| j + 12).unwrap_or(n),
        "php" => {
            let open = skip_spaces(b, name_end);
            if b.get(open) != Some(&b'(') {
                let end = find(b, name_end, b"@endphp").unwrap_or(n);
                out.put(at, ";");
                out.code(b, name_end, end);
                if end + 7 <= n {
                    out.put(end, ";");
                }
                return (end + 7).min(n);
            }
        }
        _ => {}
    }
    let open = skip_spaces(b, name_end);
    if b.get(open) != Some(&b'(') {
        return name_end; // no arguments: nothing to analyse
    }
    let Some(close) = matching_paren(b, open) else { return name_end };
    let len = name_end - at; // `@` + name
    let pad = |word: &str| -> Option<String> {
        (word.len() < len).then(|| format!(";{}{}", " ".repeat(len - 1 - word.len()), word))
    };
    let lower = name.to_ascii_lowercase();
    let head = match lower.as_str() {
        "if" | "foreach" | "for" | "while" => Some(format!(";{name}")),
        "forelse" => Some(";foreach".to_string()),
        "elseif" | "unless" => pad("if"),
        "extends" | "include" | "includeif" | "component" | "each" => pad("view"),
        "lang" | "choice" => pad("__"),
        "use" => {
            out.put(at, ";use");
            out.code(b, open + 1, close);
            for (j, &c) in b.iter().enumerate().take(close).skip(open + 1) {
                if matches!(c, b'\'' | b'"') {
                    out.bytes[j] = b' ';
                }
            }
            out.put(close, ";");
            return close + 1;
        }
        _ if lower == "php" || KEYWORD_DIRECTIVES.contains(&lower.as_str()) => {
            Some(format!(";{}", " ".repeat(len - 1)))
        }
        _ => Some(format!(";{name}")),
    };
    let Some(head) = head else { return close + 1 };
    out.put(at, &head);
    out.code(b, name_end, close + 1);
    close + 1
}

/// `:name="expr"` in a component tag; returns where scanning continues.
fn component_attribute(b: &[u8], at: usize, out: &mut Out) -> usize {
    let n = b.len();
    let mut j = at + 1;
    while j < n && (is_word(b[j]) || b[j] == b'-' || b[j] == b'.' || b[j] == b':') {
        j += 1;
    }
    if !starts(b, j, b"=\"") && !starts(b, j, b"='") {
        return j;
    }
    let q = b[j + 1];
    let start = j + 2;
    let Some(end) = (start..n).find(|&k| b[k] == q) else { return n };
    out.put(at, ";");
    out.code(b, start, end);
    out.put(end, ";");
    end + 1
}

fn is_word(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

fn starts(b: &[u8], at: usize, pat: &[u8]) -> bool {
    b.len() >= at + pat.len() && &b[at..at + pat.len()] == pat
}

fn find(b: &[u8], from: usize, pat: &[u8]) -> Option<usize> {
    (from..=b.len().checked_sub(pat.len())?).find(|&j| &b[j..j + pat.len()] == pat)
}

fn skip_spaces(b: &[u8], mut at: usize) -> usize {
    while at < b.len() && (b[at] == b' ' || b[at] == b'\t') {
        at += 1;
    }
    at
}

/// Skip a PHP string literal starting at `at` (a quote); returns the index after it.
fn skip_string(b: &[u8], at: usize) -> usize {
    let q = b[at];
    let mut j = at + 1;
    while j < b.len() && b[j] != q {
        if b[j] == b'\\' {
            j += 1;
        }
        j += 1;
    }
    (j + 1).min(b.len())
}

/// The first `close` from `at` that is outside strings and brackets.
fn close_of(b: &[u8], at: usize, close: &[u8]) -> Option<usize> {
    let mut depth = 0i32;
    let mut j = at;
    while j < b.len() {
        match b[j] {
            b'\'' | b'"' => {
                j = skip_string(b, j);
                continue;
            }
            b'(' | b'[' => depth += 1,
            b')' | b']' => depth -= 1,
            _ if depth <= 0 && starts(b, j, close) => return Some(j),
            _ => {}
        }
        j += 1;
    }
    None
}

/// The `)` matching the `(` at `open`.
fn matching_paren(b: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut j = open;
    while j < b.len() {
        match b[j] {
            b'\'' | b'"' => {
                j = skip_string(b, j);
                continue;
            }
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(j);
                }
            }
            b'\n' if depth == 0 => return None,
            _ => {}
        }
        j += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(blade: &str, php: &str) {
        let got = to_php(blade);
        assert_eq!(got.len(), blade.len());
        assert_eq!(got.trim_end(), php, "\nblade: {blade}");
    }

    #[test]
    fn echoes_and_comments() {
        check("<p>{{ $user->name }}</p>", "   ;  $user->name ;");
        check("{!! $html !!}", ";   $html ;");
        check("{{-- {{ $x }} --}}<b>@{{ $y }}</b>", "");
        check("{{ $a['}}'] }}", ";  $a['}}'] ;");
    }

    #[test]
    fn directives() {
        check("@if($a)\n<b>\n@endif", ";if($a)");
        check("@foreach ($users as $user)", ";foreach ($users as $user)");
        check("@forelse($xs as $x)", ";foreach($xs as $x)");
        check("@elseif($b)", ";    if($b)");
        check("@extends('layouts.app')", ";   view('layouts.app')");
        check("@include('a', ['x' => 1])", ";   view('a', ['x' => 1])");
        check("@lang('auth.failed')", ";  __('auth.failed')");
        check("@switch($t)", ";      ($t)");
        check("@section('title', $post->title)", ";section('title', $post->title)");
        check("@use('App\\Models\\User')", ";use  App\\Models\\User ;");
        check("a@b.com @csrf @endforeach", "");
    }

    #[test]
    fn php_blocks_and_components() {
        check("@php $x = 1; @endphp", ";    $x = 1; ;");
        check("@php($y = 2)", ";   ($y = 2)");
        check("<?php echo $z; ?>", ";     echo $z; ;");
        check("<x-alert :type=\"$kind\" class=\"{{ $c }}\" />", "         ;      $kind;        ;  $c ;");
    }

    #[test]
    fn keeps_non_ascii_and_lines() {
        let src = "<p>Çalış</p>\n{{ $ü }}";
        let php = to_php(src);
        assert_eq!(php.len(), src.len());
        assert_eq!(php.lines().count(), 2);
        assert!(php.contains('Ç') && php.contains("$ü"));
    }

    #[test]
    fn detects_blade_paths() {
        assert!(is_blade("file:///a/resources/views/home.blade.php"));
        assert!(!is_blade("file:///a/app/Http/Kernel.php"));
    }
}
