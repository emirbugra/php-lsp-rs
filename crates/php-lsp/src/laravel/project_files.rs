//! Config keys, translation keys, view names and `.env` keys of the project.

use super::{dotted, file_entry, files, lit, one_line, Entry, Reader, SourceFile};
use php_syntax::{ArrayItem, Expr, ExprKind, Span, Stmt, StmtKind};
use std::path::{Path, PathBuf};

/// `return [ ... ];` of a config / translation file.
fn returned_array<'a>(src: &str, stmts: &'a [Stmt]) -> Option<&'a [ArrayItem]> {
    for s in stmts {
        if let Some(Some(Expr { kind: ExprKind::Array(items), .. })) = s.return_value(src) {
            return Some(items);
        }
        if let StmtKind::Namespace { body: Some(b), .. } = &s.kind {
            if let Some(items) = returned_array(src, b) {
                return Some(items);
            }
        }
    }
    None
}

/// Flatten a nested PHP array literal into dotted keys.
fn array_keys(
    file: &SourceFile,
    items: &[ArrayItem],
    prefix: &str,
    locale: Option<&str>,
    out: &mut Vec<Entry>,
    depth: u32,
) {
    if depth > 16 {
        return;
    }
    for it in items {
        let Some(k) = it.key.as_ref().and_then(|k| lit(file.src, k)) else { continue };
        let key = if prefix.is_empty() { k } else { format!("{prefix}.{k}") };
        let detail = match &it.value.kind {
            ExprKind::Array(_) => "[…]".to_string(),
            _ => one_line(it.value.span.text(file.src), 80),
        };
        let mut e = file.entry(key.clone(), it.key.as_ref().unwrap().span, detail);
        e.locale = locale.map(str::to_string);
        e.open = match &it.value.kind {
            ExprKind::Lit => false,
            // a list or computed keys: indexes cannot be checked
            ExprKind::Array(sub) => sub.iter().any(|i| i.key.as_ref().and_then(|k| lit(file.src, k)).is_none()),
            _ => true,
        };
        out.push(e);
        if let ExprKind::Array(sub) = &it.value.kind {
            array_keys(file, sub, &key, locale, out, depth + 1);
        }
    }
}

pub(super) fn config(root: &Path, read: &Reader, out: &mut Vec<Entry>) {
    let dir = root.join("config");
    let mut fs = Vec::new();
    files(&dir, &mut fs, 0);
    for p in fs {
        let Some(name) = dotted(&dir, &p, ".php") else { continue };
        config_file(root, &p, &name, read, out);
    }
}

/// One config file `p`, whose keys start with `name` (`app`, `services` …).
pub(super) fn config_file(root: &Path, p: &Path, name: &str, read: &Reader, out: &mut Vec<Entry>) {
    let Some(src) = read(p) else { return };
    let rel = p.strip_prefix(root).unwrap_or(p);
    let mut whole = file_entry(p, name.to_string(), rel.display().to_string());
    let parse = php_syntax::parse(&src);
    let file = SourceFile::new(p, &src);
    match returned_array(&src, &parse.stmts) {
        Some(items) => {
            out.push(whole);
            array_keys(&file, items, name, None, out, 0);
        }
        None => {
            whole.open = true;
            out.push(whole);
        }
    }
}

pub(super) fn translations(root: &Path, read: &Reader, out: &mut Vec<Entry>) {
    for base in [root.join("lang"), root.join("resources/lang")] {
        let Ok(rd) = std::fs::read_dir(&base) else { continue };
        let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            let fname = p.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
            if p.is_dir() {
                if fname == "vendor" {
                    continue;
                }
                trans_dir(&p, &fname, read, out);
            } else if let Some(locale) = fname.strip_suffix(".json") {
                json_translations(&p, locale, read, out);
            }
        }
    }
}

/// `lang/<locale>.json`: `__('Some sentence')` keys.
fn json_translations(p: &Path, locale: &str, read: &Reader, out: &mut Vec<Entry>) {
    let Some(src) = read(p) else { return };
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(&src) else { return };
    let file = SourceFile::new(p, &src);
    for (k, v) in map {
        let quoted = serde_json::to_string(&k).unwrap_or_default();
        let at = src.find(&quoted).unwrap_or(0);
        let mut e = file.entry(k, Span::new(at, at + quoted.len()), one_line(v.as_str().unwrap_or(""), 80));
        e.locale = Some(locale.to_string());
        out.push(e);
    }
}

/// `lang/<locale>/**.php`
pub(super) fn trans_dir(dir: &Path, locale: &str, read: &Reader, out: &mut Vec<Entry>) {
    let mut fs = Vec::new();
    files(dir, &mut fs, 0);
    for f in fs {
        let Some(name) = dotted(dir, &f, ".php") else { continue };
        let Some(src) = read(&f) else { continue };
        let parse = php_syntax::parse(&src);
        let file = SourceFile::new(&f, &src);
        if let Some(items) = returned_array(&src, &parse.stmts) {
            array_keys(&file, items, &name, Some(locale), out, 0);
        }
    }
}

pub(super) fn views(root: &Path, out: &mut Vec<Entry>) {
    let dir = root.join("resources/views");
    let mut fs = Vec::new();
    files(&dir, &mut fs, 0);
    for p in fs {
        let Some(name) = dotted(&dir, &p, ".blade.php").or_else(|| dotted(&dir, &p, ".php")) else { continue };
        // resources/views/vendor/<pkg>/x.blade.php overrides `pkg::x`
        let key = match name.strip_prefix("vendor.") {
            Some(rest) => match rest.split_once('.') {
                Some((pkg, view)) => format!("{pkg}::{view}"),
                None => continue,
            },
            None => name,
        };
        let rel = p.strip_prefix(root).unwrap_or(&p).display().to_string();
        out.push(file_entry(&p, key, rel));
    }
}

pub(super) fn env(root: &Path, read: &Reader, out: &mut Vec<Entry>) {
    let p = [root.join(".env"), root.join(".env.example")].into_iter().find(|p| p.is_file());
    let Some(p) = p else { return };
    let Some(src) = read(&p) else { return };
    let file = SourceFile::new(&p, &src);
    let mut off = 0;
    for line in src.split_inclusive('\n') {
        let here = off;
        off += line.len();
        let t = line.trim_start();
        if t.starts_with('#') {
            continue;
        }
        let t = t.strip_prefix("export ").unwrap_or(t);
        let Some((k, v)) = t.split_once('=') else { continue };
        let k = k.trim();
        // CodeIgniter keys are dotted (`database.default.hostname`)
        if k.is_empty() || !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.') {
            continue;
        }
        let start = here + line.find(k).unwrap_or(0);
        out.push(file.entry(k.to_string(), Span::new(start, start + k.len()), one_line(v.trim(), 60)));
    }
}
