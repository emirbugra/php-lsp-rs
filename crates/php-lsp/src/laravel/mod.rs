//! Framework string references: route names, config keys, translation keys, view / template
//! names, controller handlers and `.env` keys of one Laravel, Symfony or CodeIgniter 4 project.
//! Cheap to rebuild (a few dozen small files), so the server keeps it for a short time only and
//! never has to track invalidation precisely.
//!
//! - `routes.rs`: Laravel route names from `routes/*.php` (groups, resources, `Auth::routes()` …)
//! - `project_files.rs`: Laravel config, translations, views and `.env`
//! - `vendor.rs`: what installed Laravel packages add (only used to avoid false warnings)
//! - `aliases.rs`: Laravel's global class aliases
//! - `symfony.rs`: Symfony routes (attributes, annotations, YAML) and Twig templates
//! - `codeigniter.rs`: CodeIgniter 4 routes, controller handlers, views, languages and configs
//! - `view_vars.rs`: the variables a view receives from the code that renders it

mod aliases;
mod codeigniter;
mod project_files;
mod routes;
mod symfony;
mod vendor;
pub mod view_vars;

pub use aliases::aliases;
pub use vendor::VendorData;

use crate::convert::*;
use lsp_types::Range;
use php_syntax::{Arg, Expr, LineIndex, Span};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct Entry {
    pub key: String,
    pub uri: String,
    pub range: Range,
    /// Short description: route method + URI, config value, translation text, file path, env value.
    pub detail: String,
    /// Translations only.
    pub locale: Option<String>,
    /// Config / translation value that is not a literal array: deeper keys cannot be checked.
    pub open: bool,
}

/// Which framework a project uses.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Framework {
    #[default]
    Laravel,
    Symfony,
    CodeIgniter,
}

/// The framework of the application at `root`, if it is one this server knows.
pub fn framework_of(root: &Path) -> Option<Framework> {
    if is_laravel(root) {
        Some(Framework::Laravel)
    } else if root.join("bin/console").is_file()
        && (root.join("config/bundles.php").is_file() || root.join("vendor/symfony/framework-bundle").is_dir())
    {
        Some(Framework::Symfony)
    } else if root.join("app/Config/Routes.php").is_file()
        && (root.join("spark").is_file() || root.join("vendor/codeigniter4/framework").is_dir())
    {
        Some(Framework::CodeIgniter)
    } else {
        None
    }
}

/// The data of one project. (Named after the first framework supported; holds every framework's.)
#[derive(Default, Debug)]
pub struct LaravelData {
    pub framework: Framework,
    pub routes: Vec<Entry>,
    pub config: Vec<Entry>,
    pub trans: Vec<Entry>,
    pub views: Vec<Entry>,
    pub env: Vec<Entry>,
    /// Controller methods a route can point at (`Home::index`; CodeIgniter).
    pub handlers: Vec<Entry>,
    /// The application finds views in places of its own (a custom `view.finder`, extra locations
    /// added in a service provider): view names cannot be checked.
    pub views_dynamic: bool,
    /// Names / keys registered by installed packages (only used to avoid false warnings).
    pub vendor: Arc<VendorData>,
}

/// What a string argument refers to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Route,
    Config,
    Trans,
    View,
    Env,
    /// A controller method in a route definition (`'Home::index'`).
    Handler,
}

impl LaravelData {
    pub fn entries(&self, k: Kind) -> &[Entry] {
        match k {
            Kind::Route => &self.routes,
            Kind::Config => &self.config,
            Kind::Trans => &self.trans,
            Kind::View => &self.views,
            Kind::Env => &self.env,
            Kind::Handler => &self.handlers,
        }
    }

    /// Entries for `key`; config keys deeper than the literal arrays fall back to the closest parent.
    pub fn lookup(&self, k: Kind, key: &str) -> Vec<&Entry> {
        let all = self.entries(k);
        let exact: Vec<&Entry> = all.iter().filter(|e| e.key == key).collect();
        if !exact.is_empty() || k != Kind::Config {
            return exact;
        }
        let mut cur = key;
        while let Some(i) = cur.rfind('.') {
            cur = &cur[..i];
            let hit: Vec<&Entry> = all.iter().filter(|e| e.key == cur).collect();
            if !hit.is_empty() {
                return hit;
            }
        }
        Vec::new()
    }
}

/// Is `root` a Laravel application?
pub fn is_laravel(root: &Path) -> bool {
    root.join("artisan").is_file() || root.join("vendor/laravel/framework").is_dir()
}

pub type Reader<'a> = dyn Fn(&Path) -> Option<String> + 'a;

pub fn build(root: &Path, read: &Reader) -> LaravelData {
    let mut d = LaravelData::default();
    match framework_of(root) {
        Some(Framework::Symfony) => {
            d.framework = Framework::Symfony;
            symfony::build(root, read, &mut d);
            project_files::env(root, read, &mut d.env);
            return d;
        }
        Some(Framework::CodeIgniter) => {
            d.framework = Framework::CodeIgniter;
            codeigniter::build(root, read, &mut d);
            project_files::env(root, read, &mut d.env);
            return d;
        }
        _ => {}
    }
    routes::routes(root, read, &mut d.routes);
    d.views_dynamic = custom_view_finder(root, read);
    project_files::config(root, read, &mut d.config);
    project_files::translations(root, read, &mut d.trans);
    project_files::views(root, &mut d.views);
    project_files::env(root, read, &mut d.env);
    d.vendor = vendor::vendor_data(root);
    d
}

/// Does a service provider replace the view finder or add view locations?
fn custom_view_finder(root: &Path, read: &Reader) -> bool {
    let mut fs = Vec::new();
    files(&root.join("app/Providers"), &mut fs, 0);
    fs.iter().filter_map(|f| read(f)).any(|src| {
        ["'view.finder'", "\"view.finder\"", "addLocation(", "prependLocation(", "setFinder("]
            .iter()
            .any(|m| src.contains(m))
    })
}

// ---- helpers shared by the collectors -------------------------------------

/// Every file below `dir`, sorted, files of a folder before its subfolders.
fn files(dir: &Path, out: &mut Vec<PathBuf>, depth: u32) {
    if depth > 12 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    let (dirs, plain): (Vec<_>, Vec<_>) = entries.into_iter().partition(|p| p.is_dir());
    out.extend(plain);
    for d in dirs {
        files(&d, out, depth + 1);
    }
}

/// Content of a string literal.
fn lit(src: &str, e: &Expr) -> Option<String> {
    e.string_value(src).map(str::to_string)
}

fn arg(args: &[Arg], i: usize) -> Option<&Expr> {
    args.get(i).map(|a| &a.value)
}

/// `s` on one line, cut to `max` characters.
fn one_line(s: &str, max: usize) -> String {
    let s: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() > max {
        format!("{}…", s.chars().take(max).collect::<String>())
    } else {
        s
    }
}

/// `a/b.php` under `dir` -> `a.b`
fn dotted(dir: &Path, path: &Path, ext: &str) -> Option<String> {
    let rel = path.strip_prefix(dir).ok()?.to_string_lossy().replace('\\', "/");
    let stem = rel.strip_suffix(ext)?;
    Some(stem.replace('/', "."))
}

/// Resolve `.` and `..` without touching the file system.
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            c => out.push(c),
        }
    }
    out
}

/// One source file being read, to turn spans into entries.
struct SourceFile<'a> {
    src: &'a str,
    lines: LineIndex,
    uri: String,
}

impl<'a> SourceFile<'a> {
    fn new(path: &Path, src: &'a str) -> Self {
        SourceFile { src, lines: LineIndex::new(src), uri: path_to_uri(path) }
    }

    fn entry(&self, key: String, span: Span, detail: String) -> Entry {
        Entry {
            key,
            uri: self.uri.clone(),
            range: range(self.src, &self.lines, span),
            detail,
            locale: None,
            open: false,
        }
    }
}

/// An entry that points at the start of a whole file (a config file, a view).
fn file_entry(path: &Path, key: String, detail: String) -> Entry {
    let zero = lsp_types::Position { line: 0, character: 0 };
    Entry { key, uri: path_to_uri(path), range: Range { start: zero, end: zero }, detail, locale: None, open: false }
}
