//! Composer awareness: autoload maps (PSR-4 / PSR-0) for the project and `vendor/`.

use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Psr4Entry {
    /// Namespace prefix with trailing backslash (may be empty).
    pub prefix: String,
    pub dir: PathBuf,
    /// true for PSR-0 style entries.
    pub psr0: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Composer {
    pub root: PathBuf,
    pub vendor_dir: PathBuf,
    pub name: Option<String>,
    pub entries: Vec<Psr4Entry>,
    /// Directories / files from `classmap` and `files` that should be indexed.
    pub extra_paths: Vec<PathBuf>,
    /// The PHP version the project targets: `config.platform.php` (`8.1.0`) or else the
    /// `require.php` constraint (`^7.4|^8.0`), as written.
    pub php: Option<String>,
}

fn string_or_array(v: &Value) -> Vec<String> {
    match v {
        Value::String(s) => vec![s.clone()],
        Value::Array(a) => a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect(),
        _ => Vec::new(),
    }
}

impl Composer {
    /// Load `dir/composer.json` plus `vendor/composer/installed.json`.
    pub fn load(dir: &Path) -> Option<Composer> {
        let text = std::fs::read_to_string(dir.join("composer.json")).ok()?;
        let json: Value = serde_json::from_str(&text).ok()?;
        let vendor = json
            .pointer("/config/vendor-dir")
            .and_then(|v| v.as_str())
            .map(|v| dir.join(v))
            .unwrap_or_else(|| dir.join("vendor"));
        let mut c = Composer {
            root: dir.to_path_buf(),
            vendor_dir: vendor.clone(),
            name: json.get("name").and_then(|n| n.as_str()).map(|s| s.to_string()),
            php: ["/config/platform/php", "/require/php"]
                .iter()
                .find_map(|p| json.pointer(p).and_then(|v| v.as_str()))
                .map(|s| s.to_string()),
            ..Default::default()
        };
        for section in ["autoload", "autoload-dev"] {
            if let Some(a) = json.get(section) {
                c.read_autoload(a, dir);
            }
        }
        c.read_installed(&vendor);
        // longest prefix first so more specific namespaces win
        c.entries.sort_by_key(|e| std::cmp::Reverse(e.prefix.len()));
        Some(c)
    }

    fn read_autoload(&mut self, a: &Value, base: &Path) {
        for (key, psr0) in [("psr-4", false), ("psr-0", true)] {
            if let Some(map) = a.get(key).and_then(|m| m.as_object()) {
                for (prefix, dirs) in map {
                    let mut prefix = prefix.clone();
                    if !prefix.is_empty() && !prefix.ends_with('\\') && !psr0 {
                        prefix.push('\\');
                    }
                    for d in string_or_array(dirs) {
                        self.entries.push(Psr4Entry {
                            prefix: prefix.clone(),
                            dir: base.join(d.trim_end_matches('/')),
                            psr0,
                        });
                    }
                }
            }
        }
        for key in ["classmap", "files"] {
            if let Some(list) = a.get(key) {
                for d in string_or_array(list) {
                    self.extra_paths.push(base.join(d.trim_end_matches('/')));
                }
            }
        }
    }

    fn read_installed(&mut self, vendor: &Path) {
        let Ok(text) = std::fs::read_to_string(vendor.join("composer/installed.json")) else { return };
        let Ok(json) = serde_json::from_str::<Value>(&text) else { return };
        let packages = match &json {
            Value::Array(a) => a.clone(),
            Value::Object(o) => o.get("packages").and_then(|p| p.as_array()).cloned().unwrap_or_default(),
            _ => Vec::new(),
        };
        for p in packages {
            let base = match p.get("install-path").and_then(|i| i.as_str()) {
                Some(rel) => normalize(&vendor.join("composer").join(rel)),
                None => match p.get("name").and_then(|n| n.as_str()) {
                    Some(n) => vendor.join(n),
                    None => continue,
                },
            };
            if let Some(a) = p.get("autoload") {
                self.read_autoload(a, &base);
            }
        }
    }

    /// Map a fully-qualified class name to its file, if the file exists.
    pub fn class_path(&self, fqn: &str) -> Option<PathBuf> {
        let fqn = fqn.trim_start_matches('\\');
        for e in &self.entries {
            if !fqn.starts_with(&e.prefix) {
                continue;
            }
            let rel = if e.psr0 {
                let (ns, class) = match fqn.rfind('\\') {
                    Some(i) => (&fqn[..i], &fqn[i + 1..]),
                    None => ("", fqn),
                };
                let mut p = ns.replace('\\', "/");
                if !p.is_empty() {
                    p.push('/');
                }
                p.push_str(&class.replace('_', "/"));
                p
            } else {
                fqn[e.prefix.len()..].replace('\\', "/")
            };
            let path = e.dir.join(format!("{rel}.php"));
            if path.is_file() {
                return Some(path);
            }
        }
        None
    }
}

fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Find composer projects at `root` and in immediate sub-directories (monorepos), skipping vendor dirs.
pub fn discover(root: &Path) -> Vec<Composer> {
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0u32)];
    while let Some((dir, depth)) = stack.pop() {
        if let Some(c) = Composer::load(&dir) {
            out.push(c);
        }
        if depth >= 3 {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name();
            let name = name.to_string_lossy();
            if p.is_dir()
                && !matches!(name.as_ref(), "vendor" | "node_modules" | ".git" | "storage" | "var" | "cache")
                && !name.starts_with('.')
            {
                stack.push((p, depth + 1));
            }
        }
    }
    out
}
