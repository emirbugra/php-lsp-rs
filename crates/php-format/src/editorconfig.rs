//! `.editorconfig`: the standard keys (`indent_style`, `indent_size`, `tab_width`, `end_of_line`,
//! `trim_trailing_whitespace`, `insert_final_newline`) and the keys PhpStorm writes for its own
//! options: `ij_<option>` for options of every language and `ij_php_<option>` for PHP
//! (`ij_php_concat_spaces = false`, `ij_continuation_indent_size = 8`).
//!
//! Reading a project's settings for one file: [`EditorConfig::parse`] each `.editorconfig` from
//! the file's directory up to the one with `root = true`, then [`EditorConfig::apply`] them from
//! the top down.

use crate::options::FormatOptions;
use crate::scheme::{apply as apply_option, section_of, Imported, Section};

/// One parsed `.editorconfig` file.
#[derive(Clone, Debug, Default)]
pub struct EditorConfig {
    /// `root = true`: files further up are not read.
    pub root: bool,
    /// `[glob]` sections with their `key = value` pairs (keys lower case), in file order.
    pub sections: Vec<(String, Vec<(String, String)>)>,
}

impl EditorConfig {
    pub fn parse(text: &str) -> EditorConfig {
        let mut ec = EditorConfig::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            if let Some(glob) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                ec.sections.push((glob.to_string(), Vec::new()));
                continue;
            }
            let Some((key, value)) = line.split_once('=') else { continue };
            let (key, value) = (key.trim().to_ascii_lowercase(), value.trim().to_string());
            match ec.sections.last_mut() {
                Some((_, pairs)) => pairs.push((key, value)),
                None if key == "root" => ec.root = value.eq_ignore_ascii_case("true"),
                None => {}
            }
        }
        ec
    }

    /// The `key = value` pairs that apply to `path` (relative to this file's directory, with `/`),
    /// later sections winning.
    pub fn pairs_for(&self, path: &str) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        for (glob, pairs) in &self.sections {
            if !glob_matches(glob, path) {
                continue;
            }
            for (k, v) in pairs {
                match out.iter_mut().find(|(ok, _)| ok == k) {
                    Some(slot) => slot.1 = v.clone(),
                    None => out.push((k.clone(), v.clone())),
                }
            }
        }
        out
    }

    /// Apply the settings for `path` on top of `o`.
    pub fn apply(&self, path: &str, o: &mut FormatOptions, unsupported: &mut Vec<String>, invalid: &mut Vec<String>) {
        apply_pairs(&self.pairs_for(path), o, unsupported, invalid);
    }

    /// Read an `.editorconfig` on its own: the settings it gives a PHP file, on top of PhpStorm's
    /// defaults (what PhpStorm itself would use).
    pub fn import(text: &str) -> Imported {
        let ec = EditorConfig::parse(text);
        let mut options = FormatOptions::phpstorm();
        let (mut unsupported, mut invalid) = (Vec::new(), Vec::new());
        ec.apply("file.php", &mut options, &mut unsupported, &mut invalid);
        Imported { options, name: None, unsupported, invalid }
    }
}

fn apply_pairs(
    pairs: &[(String, String)],
    o: &mut FormatOptions,
    unsupported: &mut Vec<String>,
    invalid: &mut Vec<String>,
) {
    let get = |k: &str| pairs.iter().rev().find(|(pk, _)| pk == k).map(|(_, v)| v.as_str());
    let mut bad = |k: &str, v: &str| invalid.push(format!("{k} = {v}"));
    // standard keys
    if let Some(v) = get("tab_width") {
        if !o.set("TAB_SIZE", v) {
            bad("tab_width", v);
        }
    }
    match get("indent_style") {
        Some(v) if v.eq_ignore_ascii_case("tab") => o.USE_TAB_CHARACTER = true,
        Some(v) if v.eq_ignore_ascii_case("space") => o.USE_TAB_CHARACTER = false,
        Some(v) => bad("indent_style", v),
        None => {}
    }
    match get("indent_size") {
        Some(v) if v.eq_ignore_ascii_case("tab") => {
            o.INDENT_SIZE = o.TAB_SIZE;
        }
        Some(v) => {
            if o.set("INDENT_SIZE", v) {
                // `tab_width` defaults to `indent_size`
                if get("tab_width").is_none() {
                    o.TAB_SIZE = o.INDENT_SIZE;
                }
            } else {
                bad("indent_size", v);
            }
        }
        None => {}
    }
    match get("end_of_line").map(str::to_ascii_lowercase).as_deref() {
        Some("lf") => o.LINE_SEPARATOR = "\n".into(),
        Some("crlf") => o.LINE_SEPARATOR = "\r\n".into(),
        Some(v) => bad("end_of_line", v),
        None => {}
    }
    match get("max_line_length") {
        Some(v) if v.eq_ignore_ascii_case("off") => {}
        Some(v) if !o.set("RIGHT_MARGIN", v) => bad("max_line_length", v),
        Some(_) => {}
        None => {}
    }
    for (key, option) in
        [("trim_trailing_whitespace", "STRIP_TRAILING_SPACES"), ("insert_final_newline", "ENSURE_NEWLINE_AT_EOF")]
    {
        if let Some(v) = get(key) {
            if !o.set(option, &v.to_ascii_lowercase()) {
                bad(key, v);
            }
        }
    }
    // PhpStorm's keys: `ij_<root / indent option>` first, `ij_php_<option>` wins over it
    let mut php = Vec::new();
    for (key, value) in pairs {
        if let Some(name) = key.strip_prefix("ij_php_") {
            php.push((name.to_ascii_uppercase(), value));
        } else if let Some(name) = key.strip_prefix("ij_") {
            let name = name.to_ascii_uppercase();
            // other languages' keys (`ij_javascript_…`) are not ours
            if matches!(section_of(&name), Section::Root | Section::Indent)
                || FormatOptions::KEYS.contains(&name.as_str())
            {
                apply_option(o, &name, value, unsupported, invalid);
            }
        }
    }
    for (name, value) in php {
        apply_option(o, &name, value, unsupported, invalid);
    }
}

/// `.editorconfig` text for these options: the standard keys plus `ij_*` / `ij_php_*` for every
/// option, which PhpStorm reads too.
pub fn to_editorconfig(o: &FormatOptions) -> String {
    let mut out = String::from("[{*.php,*.phtml,*.inc}]\n");
    out += &format!("indent_style = {}\n", if o.USE_TAB_CHARACTER { "tab" } else { "space" });
    out += &format!("indent_size = {}\n", o.INDENT_SIZE);
    out += &format!("tab_width = {}\n", o.TAB_SIZE);
    match o.LINE_SEPARATOR.as_str() {
        "\n" => out += "end_of_line = lf\n",
        "\r\n" => out += "end_of_line = crlf\n",
        _ => {}
    }
    out += &format!("max_line_length = {}\n", o.RIGHT_MARGIN);
    out += &format!("trim_trailing_whitespace = {}\n", o.STRIP_TRAILING_SPACES);
    out += &format!("insert_final_newline = {}\n", o.ENSURE_NEWLINE_AT_EOF);
    for key in FormatOptions::KEYS {
        let prefix = match (section_of(key), *key) {
            (Section::Editor, _)
            | (_, "INDENT_SIZE" | "TAB_SIZE" | "USE_TAB_CHARACTER" | "LINE_SEPARATOR" | "RIGHT_MARGIN") => continue,
            (Section::Root | Section::Indent, _) => "ij_",
            _ => "ij_php_",
        };
        let value = o.get(key).unwrap_or_default();
        out += &format!("{prefix}{} = {value}\n", key.to_ascii_lowercase());
    }
    out
}

/// Does `.editorconfig` section glob `glob` match `path` (relative, `/`-separated)? A glob
/// without `/` matches the file name in any directory.
pub fn glob_matches(glob: &str, path: &str) -> bool {
    let glob =
        if glob.contains('/') { glob.strip_prefix('/').unwrap_or(glob).to_string() } else { format!("**/{glob}") };
    let path = path.strip_prefix('/').unwrap_or(path);
    expand_braces(&glob).iter().any(|g| matches(g.as_bytes(), path.as_bytes()))
}

/// `a{b,c}d` → `abd`, `acd` (nested braces too). A brace without a comma is literal.
fn expand_braces(glob: &str) -> Vec<String> {
    let b = glob.as_bytes();
    let mut depth = 0;
    let mut open = None;
    for (i, &c) in b.iter().enumerate() {
        match c {
            b'{' => {
                if depth == 0 {
                    open = Some(i);
                }
                depth += 1;
            }
            b'}' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    let start = open.expect("set with depth");
                    let inner = &glob[start + 1..i];
                    // split on top-level commas
                    let mut parts = Vec::new();
                    let (mut d, mut last) = (0, 0);
                    for (j, ch) in inner.char_indices() {
                        match ch {
                            '{' => d += 1,
                            '}' => d -= 1,
                            ',' if d == 0 => {
                                parts.push(&inner[last..j]);
                                last = j + 1;
                            }
                            _ => {}
                        }
                    }
                    parts.push(&inner[last..]);
                    if parts.len() < 2 {
                        continue;
                    }
                    let (pre, post) = (&glob[..start], &glob[i + 1..]);
                    return parts.iter().flat_map(|p| expand_braces(&format!("{pre}{p}{post}"))).collect();
                }
            }
            _ => {}
        }
    }
    vec![glob.to_string()]
}

/// Glob match with `*` (not `/`), `**` (anything), `?`, `[abc]`, `[!a-z]`.
fn matches(g: &[u8], p: &[u8]) -> bool {
    match g.first() {
        None => p.is_empty(),
        Some(b'*') if g.get(1) == Some(&b'*') => {
            let rest = &g[2..];
            // `**/x` also matches `x` at the top
            if rest.first() == Some(&b'/') && matches(&rest[1..], p) {
                return true;
            }
            (0..=p.len()).any(|i| matches(rest, &p[i..]))
        }
        Some(b'*') => {
            let rest = &g[1..];
            for i in 0..=p.len() {
                if matches(rest, &p[i..]) {
                    return true;
                }
                if i < p.len() && p[i] == b'/' {
                    break;
                }
            }
            false
        }
        Some(b'?') => !p.is_empty() && p[0] != b'/' && matches(&g[1..], &p[1..]),
        Some(b'[') => {
            let Some(close) = g.iter().skip(1).position(|&c| c == b']').map(|i| i + 1) else {
                return !p.is_empty() && p[0] == b'[' && matches(&g[1..], &p[1..]);
            };
            if p.is_empty() {
                return false;
            }
            let (negate, set) = match g[1] {
                b'!' | b'^' => (true, &g[2..close]),
                _ => (false, &g[1..close]),
            };
            let c = p[0];
            let mut hit = false;
            let mut i = 0;
            while i < set.len() {
                if i + 2 < set.len() && set[i + 1] == b'-' {
                    hit |= set[i] <= c && c <= set[i + 2];
                    i += 3;
                } else {
                    hit |= set[i] == c;
                    i += 1;
                }
            }
            hit != negate && matches(&g[close + 1..], &p[1..])
        }
        Some(&c) => !p.is_empty() && p[0] == c && matches(&g[1..], &p[1..]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        assert!(glob_matches("*", "src/a.php"));
        assert!(glob_matches("*.php", "src/deep/a.php"));
        assert!(glob_matches("{*.php,*.phtml}", "a.phtml"));
        assert!(glob_matches("*.{php,inc}", "x/a.inc"));
        assert!(!glob_matches("*.js", "a.php"));
        assert!(glob_matches("src/**.php", "src/a/b.php"));
        assert!(!glob_matches("src/*.php", "src/a/b.php"));
        assert!(glob_matches("/src/*.php", "src/b.php"));
        assert!(glob_matches("a?.php", "ab.php"));
        assert!(glob_matches("[ab].php", "b.php"));
        assert!(!glob_matches("[!ab].php", "b.php"));
    }
}
