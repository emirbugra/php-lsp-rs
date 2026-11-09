//! The string literal under the cursor and the framework call it is an argument of.

use crate::laravel::{Framework, Kind};

/// The string literal under the cursor.
#[derive(Debug, Clone)]
pub struct StrLit {
    /// Byte offset of the opening quote.
    pub quote: usize,
    /// Content range (without quotes); `end` is the closing quote or the end of the line.
    pub start: usize,
    pub end: usize,
}

/// Plain (non-interpolated) string literal containing `offset`, found by scanning its line.
pub fn string_at(text: &str, offset: usize) -> Option<StrLit> {
    let b = text.as_bytes();
    let offset = offset.min(b.len());
    let line_start = text[..offset].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let mut i = line_start;
    let mut open: Option<(usize, u8)> = None;
    while i < offset {
        let c = b[i];
        match open {
            Some((_, q)) => {
                if c == b'\\' {
                    i += 1;
                } else if c == q {
                    open = None;
                }
            }
            None => match c {
                b'\'' | b'"' => open = Some((i, c)),
                b'#' => return None,
                b'/' if b.get(i + 1) == Some(&b'/') || b.get(i + 1) == Some(&b'*') => return None,
                _ => {}
            },
        }
        i += 1;
    }
    let (quote, q) = open?;
    if q == b'"' && text[quote + 1..offset].contains('$') {
        return None;
    }
    let mut end = offset;
    while end < b.len() && b[end] != b'\n' {
        if b[end] == b'\\' {
            end += 2;
            continue;
        }
        if b[end] == q {
            break;
        }
        end += 1;
    }
    Some(StrLit { quote, start: quote + 1, end: end.min(b.len()) })
}

/// The unmatched `(` before `pos` and the index of the argument `pos` is in.
pub(crate) fn open_paren(text: &str, pos: usize) -> Option<(usize, usize)> {
    let b = text.as_bytes();
    let (mut depth, mut commas, mut i, mut steps) = (0i32, 0usize, pos, 0);
    while i > 0 && steps < 2000 {
        i -= 1;
        steps += 1;
        match b[i] {
            b')' | b']' | b'}' => depth += 1,
            b'(' | b'[' | b'{' if depth > 0 => depth -= 1,
            b'(' => return Some((i, commas)),
            b'[' | b'{' | b';' => return None,
            b',' if depth == 0 => commas += 1,
            _ => {}
        }
    }
    None
}

pub(crate) fn word_before(text: &str, end: usize) -> (usize, &str) {
    let b = text.as_bytes();
    let mut e = end;
    while e > 0 && b[e - 1].is_ascii_whitespace() {
        e -= 1;
    }
    let mut s = e;
    while s > 0 && (b[s - 1].is_ascii_alphanumeric() || b[s - 1] == b'_' || b[s - 1] == b'\\') {
        s -= 1;
    }
    (s, &text[s..e])
}

/// How a call uses its key: existence checks and setters must not be reported as unknown keys.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Use {
    Read,
    Check,
    Set,
}

fn use_of(method: &str) -> Use {
    match method {
        "has" | "exists" | "is" | "routeis" | "named" | "currentroutenamed" => Use::Check,
        "set" => Use::Set,
        _ => Use::Read,
    }
}

/// Which registry a string argument at `lit` refers to.
pub fn kind_at(text: &str, lit: &StrLit, fw: Framework) -> Option<Kind> {
    call_at(text, lit, fw).map(|(k, _)| k)
}

/// Registry and usage of the string argument at `lit`.
pub fn call_at(text: &str, lit: &StrLit, fw: Framework) -> Option<(Kind, Use)> {
    let (paren, arg_index) = open_paren(text, lit.quote)?;
    let (word_start, word) = word_before(text, paren);
    let word = word.trim_start_matches('\\').to_ascii_lowercase();
    let before = text[..word_start].trim_end();
    match fw {
        Framework::Laravel => laravel_call(text, before, &word, arg_index),
        Framework::Symfony => symfony_call(before, &word, arg_index),
        Framework::CodeIgniter => codeigniter_call(before, &word, arg_index),
    }
}

/// `$this->generateUrl('x')`, `$router->generate('x')`, `$this->render('x.html.twig')`,
/// `#[Template('x.html.twig')]`.
fn symfony_call(before: &str, word: &str, arg_index: usize) -> Option<(Kind, Use)> {
    if arg_index != 0 {
        return None;
    }
    let kind = if let Some(receiver) = before.strip_suffix("->") {
        let receiver = receiver.trim_end_matches('?').trim_end().to_ascii_lowercase();
        match word {
            "generateurl" | "redirecttoroute" => Some(Kind::Route),
            // `generate()` of anything else (UUID factories, fakers) is not a route
            "generate" if receiver.ends_with("router") || receiver.ends_with("generator") => Some(Kind::Route),
            "render" | "renderview" | "renderblock" | "renderblockview" | "stream" => Some(Kind::View),
            _ => None,
        }
    } else if before.ends_with("::") {
        None
    } else {
        (word == "template").then_some(Kind::View)
    };
    kind.map(|k| (k, Use::Read))
}

/// `view('x')`, `route_to('x')`, `lang('File.key')`, `config('App')`, `env('X')`,
/// `redirect()->route('x')`, and the handler of `$routes->get('path', 'Home::index')`.
fn codeigniter_call(before: &str, word: &str, arg_index: usize) -> Option<(Kind, Use)> {
    if let Some(receiver) = before.strip_suffix("->") {
        let receiver = receiver.trim_end_matches('?').trim_end();
        let kind = match (word, arg_index) {
            ("route", 0) => Some(Kind::Route),
            ("get" | "post" | "put" | "patch" | "delete" | "options" | "head" | "add" | "cli", 1)
                if receiver.ends_with("$routes") =>
            {
                Some(Kind::Handler)
            }
            ("match", 2) if receiver.ends_with("$routes") => Some(Kind::Handler),
            _ => None,
        };
        return kind.map(|k| (k, Use::Read));
    }
    if before.ends_with("::") {
        return None;
    }
    let kind = match (word, arg_index) {
        ("view", 0) => Some(Kind::View),
        ("route_to" | "url_to", 0) => Some(Kind::Route),
        ("lang", 0) => Some(Kind::Trans),
        ("config", 0) => Some(Kind::Config),
        ("env", 0) => Some(Kind::Env),
        _ => None,
    };
    kind.map(|k| (k, Use::Read))
}

fn laravel_call(text: &str, before: &str, word: &str, arg_index: usize) -> Option<(Kind, Use)> {
    let word = word.to_string();
    if before.ends_with("->") {
        // method call: `->route('x')`, `->view('x')`, `config()->get('x')`
        let receiver = before.trim_end_matches("->").trim_end_matches('?').trim_end();
        let kind = match (word.as_str(), arg_index) {
            ("route" | "signedroute" | "temporarysignedroute" | "routeis" | "named" | "currentroutenamed", 0) => {
                Some(Kind::Route)
            }
            ("view" | "markdown", 0) => Some(Kind::View),
            ("get" | "has" | "set" | "string" | "integer" | "boolean" | "array", 0)
                if receiver.ends_with("config()") =>
            {
                Some(Kind::Config)
            }
            ("get" | "has" | "choice", 0) if receiver.ends_with("app('translator')") => Some(Kind::Trans),
            _ => None,
        };
        return kind.map(|k| (k, use_of(&word)));
    }
    if before.ends_with("::") {
        let (_, class) = word_before(text, before.len() - 2);
        let class = class.rsplit('\\').next().unwrap_or("").to_ascii_lowercase();
        let kind = match (class.as_str(), word.as_str(), arg_index) {
            ("route", "has" | "is" | "currentroutenamed", 0) => Some(Kind::Route),
            ("route", "view", 1) => Some(Kind::View),
            ("url" | "redirect", "route" | "signedroute" | "temporarysignedroute", 0) => Some(Kind::Route),
            ("config", "get" | "has" | "string" | "integer" | "boolean" | "array" | "set", 0) => Some(Kind::Config),
            ("view", "make" | "exists" | "first", 0) => Some(Kind::View),
            ("lang", "get" | "has" | "choice", 0) => Some(Kind::Trans),
            _ => None,
        };
        return kind.map(|k| (k, use_of(&word)));
    }
    let kind = match (word.as_str(), arg_index) {
        ("route" | "to_route", 0) => Some(Kind::Route),
        ("config", 0) => Some(Kind::Config),
        ("view", 0) => Some(Kind::View),
        ("__" | "trans" | "trans_choice", 0) => Some(Kind::Trans),
        ("env", 0) => Some(Kind::Env),
        _ => None,
    };
    kind.map(|k| (k, Use::Read))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(src: &str) -> Option<Kind> {
        let at = src.find('|').unwrap();
        let text = src.replace('|', "");
        let lit = string_at(&text, at)?;
        kind_at(&text, &lit, Framework::Laravel)
    }

    #[test]
    fn detects_call_kinds() {
        assert_eq!(kind("<?php route('us|ers.index');"), Some(Kind::Route));
        assert_eq!(kind("<?php return redirect()->route('ho|me', [1]);"), Some(Kind::Route));
        assert_eq!(kind("<?php \\Illuminate\\Support\\Facades\\Route::has('x|');"), Some(Kind::Route));
        assert_eq!(kind("<?php config('app.na|me');"), Some(Kind::Config));
        assert_eq!(kind("<?php config()->get('app|');"), Some(Kind::Config));
        assert_eq!(kind("<?php Config::get('a|');"), Some(Kind::Config));
        assert_eq!(kind("<?php view('users.|index', ['a' => 1]);"), Some(Kind::View));
        assert_eq!(kind("<?php Route::view('/x', 'welc|ome');"), Some(Kind::View));
        assert_eq!(kind("<?php __(\"error.|x\");"), Some(Kind::Trans));
        assert_eq!(kind("<?php trans('a.b|');"), Some(Kind::Trans));
        assert_eq!(kind("<?php env('APP_|KEY', 'x');"), Some(Kind::Env));
        assert_eq!(kind("<?php env('APP_KEY', 'x|');"), None);
        assert_eq!(kind("<?php foo('x|');"), None);
        assert_eq!(kind("<?php route(\"a.$b|\");"), None);
        assert_eq!(kind("<?php // route('x|')"), None);
    }
}
