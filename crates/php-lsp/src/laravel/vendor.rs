//! What `vendor/` contributes: package route names, package config defaults (`mergeConfigFrom`)
//! and the framework's own translations.

use super::project_files::{config_file, trans_dir};
use super::routes::{RouteGroup, RouteWalker};
use super::{dotted, files, Entry, SourceFile};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

/// Cached until `vendor/composer/installed.json` changes.
#[derive(Default, Debug)]
pub struct VendorData {
    pub routes: Vec<String>,
    pub config: Vec<Entry>,
    pub trans: Vec<Entry>,
}

/// Per project root: (`installed.json` modification time, scanned data).
type VendorCache = Mutex<HashMap<PathBuf, (Option<SystemTime>, Arc<VendorData>)>>;

pub(super) fn vendor_data(root: &Path) -> Arc<VendorData> {
    static CACHE: OnceLock<VendorCache> = OnceLock::new();
    let stamp = std::fs::metadata(root.join("vendor/composer/installed.json")).and_then(|m| m.modified()).ok();
    let cache = CACHE.get_or_init(Default::default);
    if let Some((at, d)) = cache.lock().unwrap().get(root) {
        if *at == stamp {
            return d.clone();
        }
    }
    let d = Arc::new(scan_vendor(root));
    cache.lock().unwrap().insert(root.to_path_buf(), (stamp, d.clone()));
    d
}

fn scan_vendor(root: &Path) -> VendorData {
    let mut d = VendorData::default();
    let vendor = root.join("vendor");
    let read = |p: &Path| std::fs::read_to_string(p).ok();
    let mut packages = Vec::new();
    for v in std::fs::read_dir(&vendor).into_iter().flatten().flatten() {
        if v.path().is_dir() && v.file_name() != "composer" && v.file_name() != "bin" {
            packages.extend(std::fs::read_dir(v.path()).into_iter().flatten().flatten().map(|p| p.path()));
        }
    }
    packages.sort();
    let mut route_out = Vec::new();
    for pkg in &packages {
        // package defaults merged through `mergeConfigFrom(__DIR__.'/../config/x.php', 'x')`
        let cdir = pkg.join("config");
        for p in std::fs::read_dir(&cdir).into_iter().flatten().flatten().map(|e| e.path()) {
            if let Some(name) = dotted(&cdir, &p, ".php") {
                config_file(root, &p, &name, &read, &mut d.config);
            }
        }
        package_routes(root, pkg, &mut route_out);
    }
    d.routes = route_out.into_iter().map(|e| e.key).collect();
    // Laravel 11+ serves the local disk under `storage.local`
    d.routes.push("storage.local".into());
    // framework translations (`validation.*`, `auth.*` …) that a published `lang/` overrides key by key
    let tdir = vendor.join("laravel/framework/src/Illuminate/Translation/lang");
    for loc in std::fs::read_dir(&tdir).into_iter().flatten().flatten().map(|e| e.path()) {
        let locale = loc.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        trans_dir(&loc, &locale, &read, &mut d.trans);
    }
    d
}

/// Routes of one package: `loadRoutesFrom(...)` files and providers that register routes directly.
fn package_routes(root: &Path, pkg: &Path, out: &mut Vec<Entry>) {
    let mut fs = Vec::new();
    files(&pkg.join("routes"), &mut fs, 0);
    for sub in ["src", "src/Providers"] {
        for p in std::fs::read_dir(pkg.join(sub)).into_iter().flatten().flatten().map(|e| e.path()) {
            if p.to_string_lossy().ends_with("ServiceProvider.php") {
                fs.push(p);
            }
        }
    }
    for p in fs.into_iter().filter(|p| p.extension().is_some_and(|e| e == "php")) {
        let Ok(src) = std::fs::read_to_string(&p) else { continue };
        if !src.contains("Route") {
            continue;
        }
        let parse = php_syntax::parse(&src);
        let mut groups = HashMap::new();
        let mut w = RouteWalker {
            root,
            path: &p,
            source: SourceFile::new(&p, &src),
            group: RouteGroup::default(),
            out: &mut *out,
            file_groups: &mut groups,
        };
        php_syntax::walk::walk_stmts(&mut w, &parse.stmts);
    }
}
