//! Route / view / config / translation names that do not exist in the project.

use super::strings::{call_at, StrLit, Use};
use crate::laravel::{Entry, Framework, Kind, LaravelData};
use php_syntax::walk::{walk_expr, Visitor};
use php_syntax::{ClassRef, Expr, ExprKind, MemberName, Stmt};

/// A Laravel string argument that names nothing in the project.
#[derive(Debug, Clone)]
pub struct Unknown {
    /// Content range (without quotes).
    pub start: usize,
    pub end: usize,
    pub kind: Kind,
    pub key: String,
    pub suggestion: Option<String>,
}

/// String literals of a document, plus config keys it sets at runtime (`config(['a.b' => 1])`).
struct Strings<'a> {
    src: &'a str,
    lits: Vec<(usize, usize)>,
    config_sets: Vec<String>,
}

impl<'ast> Visitor<'ast> for Strings<'_> {
    fn visit_expr(&mut self, e: &'ast Expr) {
        if matches!(e.kind, ExprKind::Lit) {
            self.lits.push((e.span.start as usize, e.span.end as usize));
        }
        let set_args = match &e.kind {
            ExprKind::Call { callee, args } => match &callee.kind {
                ExprKind::Name(n) if n.text.trim_start_matches('\\').eq_ignore_ascii_case("config") => Some(args),
                _ => None,
            },
            ExprKind::StaticCall { class: ClassRef::Name(c), name: MemberName::Ident(id), args }
                if c.text.rsplit('\\').next().is_some_and(|c| c.eq_ignore_ascii_case("Config"))
                    && id.text.eq_ignore_ascii_case("set") =>
            {
                Some(args)
            }
            _ => None,
        };
        if let Some(ExprKind::Array(items)) = set_args.and_then(|a| a.first()).map(|a| &a.value.kind) {
            for it in items {
                if let Some(k) = it.key.as_ref().and_then(|k| php_syntax::string_content(k.span.text(self.src))) {
                    self.config_sets.push(k.to_string());
                }
            }
        }
        walk_expr(self, e);
    }
}

/// Is `key` (or a parent of it with unknown children) among `entries`? Keys whose top level is not
/// known at all are accepted: they come from somewhere this server does not read.
fn nested_known<'e>(key: &str, entries: impl Iterator<Item = &'e Entry> + Clone) -> bool {
    if entries.clone().any(|e| e.key == key) {
        return true;
    }
    let mut cur = key;
    while let Some(i) = cur.rfind('.') {
        cur = &cur[..i];
        let mut parents = entries.clone().filter(|e| e.key == cur).peekable();
        if parents.peek().is_some() {
            return parents.any(|e| e.open);
        }
        // a translation group (`messages.php`) has no entry of its own, only its keys
        let prefix = format!("{cur}.");
        if entries.clone().any(|e| e.key.starts_with(&prefix)) {
            return false;
        }
    }
    true
}

fn known(data: &LaravelData, kind: Kind, key: &str, config_sets: &[String]) -> bool {
    match data.framework {
        Framework::Laravel => {}
        Framework::Symfony => {
            return match kind {
                // `_wdt`, `_profiler`, `_api_*`: routes of bundles
                Kind::Route => {
                    data.routes.is_empty() || key.starts_with('_') || data.routes.iter().any(|e| e.key == key)
                }
                // `@Bundle/x.html.twig`; plain strings that are not templates (`->render('x')` of something else)
                Kind::View => {
                    data.views.is_empty()
                        || key.starts_with('@')
                        || !key.ends_with(".twig")
                        || data.views.iter().any(|e| e.key == key)
                }
                _ => true,
            };
        }
        Framework::CodeIgniter => {
            return match kind {
                // routes can be named by their handler (`route_to('Home::index')`)
                Kind::Route => data.routes.is_empty() || key.contains("::") || data.routes.iter().any(|e| e.key == key),
                // namespaced views (`Modules\Blog\Views\x`) live elsewhere
                Kind::View => data.views.is_empty() || key.contains('\\') || data.views.iter().any(|e| e.key == key),
                Kind::Trans => {
                    key.contains(char::is_whitespace) || !key.contains('.') || nested_known(key, data.trans.iter())
                }
                // config classes of other namespaces (packages) are found at runtime
                _ => true,
            };
        }
    }
    match kind {
        Kind::Env | Kind::Handler => true,
        Kind::Route => {
            data.routes.is_empty()
                || data.routes.iter().any(|e| e.key == key)
                // package routes whose group prefix lives in code this server does not follow
                || data.vendor.routes.iter().any(|n| n == key || key.strip_suffix(n.as_str()).is_some_and(|p| p.ends_with('.')))
        }
        Kind::View => {
            let key = key.replace('/', ".");
            data.views.is_empty() || data.views_dynamic || key.contains("::") || data.views.iter().any(|e| e.key == key)
        }
        Kind::Config => {
            // `App::setLocale()` writes `app.locale` at runtime
            key == "app.locale"
                || config_sets
                    .iter()
                    .any(|s| s == key || key.starts_with(&format!("{s}.")) || s.starts_with(&format!("{key}.")))
                || nested_known(key, data.config.iter().chain(&data.vendor.config))
        }
        Kind::Trans => {
            // `__('Plain sentence')` falls back to the key itself; only `group.key` forms are checked
            key.contains(char::is_whitespace)
                || key.contains("::")
                || !key.contains('.')
                || key.ends_with('.')
                || nested_known(key, data.trans.iter().chain(&data.vendor.trans))
        }
    }
}

/// Optimal string alignment distance (Levenshtein with adjacent transpositions).
fn distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[a.len()][b.len()]
}

fn suggestion(data: &LaravelData, kind: Kind, key: &str) -> Option<String> {
    let limit = (key.chars().count() / 4).clamp(1, 3);
    let vendor: &[Entry] = match kind {
        Kind::Config => &data.vendor.config,
        Kind::Trans => &data.vendor.trans,
        _ => &[],
    };
    let mut keys: Vec<&str> = data.entries(kind).iter().chain(vendor).map(|e| e.key.as_str()).collect();
    if kind == Kind::Route {
        keys.extend(data.vendor.routes.iter().map(String::as_str));
    }
    let lookup = if kind == Kind::View { key.replace('/', ".") } else { key.to_string() };
    keys.into_iter()
        .filter(|k| k.len().abs_diff(lookup.len()) <= limit)
        .map(|k| (distance(&lookup, k), k))
        .filter(|(d, _)| *d <= limit)
        .min()
        .map(|(_, k)| k.to_string())
}

/// Route / view / config / translation keys of `text` that are not defined in the project.
pub fn unknown_keys(text: &str, stmts: &[Stmt], data: &LaravelData) -> Vec<Unknown> {
    let mut v = Strings { src: text, lits: Vec::new(), config_sets: Vec::new() };
    php_syntax::walk::walk_stmts(&mut v, stmts);
    let mut calls = Vec::new();
    for (s, e) in v.lits {
        let Some(content) = php_syntax::string_content(&text[s..e]) else { continue };
        if content.is_empty() || content.contains(['\\', '{', '*', '$', '\n']) {
            continue;
        }
        // the literal must be the whole argument: not `route('a.' . $b)` or `view($a ? 'x' : 'y')`
        let prev = text[..s].trim_end().bytes().last();
        let next = text[e..].trim_start().bytes().next();
        if !matches!(prev, Some(b'(' | b',')) || !matches!(next, Some(b')' | b',')) {
            continue;
        }
        let lit = StrLit { quote: s, start: s + 1, end: e - 1 };
        let Some((kind, usage)) = call_at(text, &lit, data.framework) else { continue };
        if usage == Use::Set && kind == Kind::Config {
            v.config_sets.push(content.to_string());
        }
        if usage == Use::Read {
            calls.push((lit, kind, content.to_string()));
        }
    }
    calls
        .into_iter()
        .filter(|(_, kind, key)| !known(data, *kind, key, &v.config_sets))
        .map(|(lit, kind, key)| Unknown {
            start: lit.start,
            end: lit.end,
            kind,
            suggestion: suggestion(data, kind, &key),
            key,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::Range;

    fn entry(key: &str, open: bool) -> Entry {
        Entry {
            key: key.into(),
            uri: String::new(),
            range: Range::default(),
            detail: String::new(),
            locale: None,
            open,
        }
    }

    #[test]
    fn reports_unknown_keys() {
        let data = LaravelData {
            routes: vec![entry("users.show", false)],
            config: vec![entry("app", false), entry("app.name", false), entry("database.options", true)],
            trans: vec![entry("error", false), entry("error.auth", false), entry("error.auth.failed", false)],
            views: vec![entry("users.index", false)],
            ..Default::default()
        };
        let src = r#"<?php
route('users.shwo');
route('users.show');
Route::has('nope');
request()->routeIs('admin.*');
route('a.' . $b);
config('app.nmae');
config('app.name');
config('database.options.x');
config('mail.from');
config(['app.custom' => 1]);
config('app.custom.deep');
Config::set('app.other', 1);
config('app.other');
view('users.index');
view('users/index');
view('users.edit');
view('mail::message');
__('error.auth.faild');
__('error.auth.failed');
__('Welcome back.');
__('Some sentence. With dots');
__('unknown_group.key');
env('NOT_THERE');
"#;
        let parse = php_syntax::parse(src);
        let got: Vec<(String, Option<String>)> =
            unknown_keys(src, &parse.stmts, &data).into_iter().map(|u| (u.key, u.suggestion)).collect();
        let s = |k: &str, sug: Option<&str>| (k.to_string(), sug.map(str::to_string));
        assert_eq!(
            got,
            vec![
                s("users.shwo", Some("users.show")),
                s("app.nmae", Some("app.name")),
                s("users.edit", None),
                s("error.auth.faild", Some("error.auth.failed")),
            ]
        );
    }
}
