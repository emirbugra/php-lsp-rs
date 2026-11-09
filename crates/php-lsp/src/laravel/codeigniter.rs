//! CodeIgniter 4: route names (`['as' => 'name']` in `app/Config/Routes.php`), controller handlers
//! (`Admin\Users::index`, the public methods of the classes under `app/Controllers`), views
//! (`app/Views/users/index.php` → `users/index`), language keys (`app/Language/<locale>/File.php`
//! → `File.key`, plus the framework's and packages' own) and config classes (`app/Config/App.php`
//! → `App`).

use super::project_files::trans_dir;
use super::{file_entry, files, Entry, LaravelData, Reader, SourceFile};
use php_syntax::walk::{walk_expr, walk_stmts, Visitor};
use php_syntax::{Expr, ExprKind, Member, MemberName, Stmt, StmtKind, Visibility};
use std::path::Path;

/// Route methods of `RouteCollection` whose second argument is the handler.
const ROUTE_METHODS: &[&str] = &["get", "post", "put", "patch", "delete", "options", "head", "add", "cli"];

pub(super) fn build(root: &Path, read: &Reader, d: &mut LaravelData) {
    let mut route_files = vec![root.join("app/Config/Routes.php")];
    files(&root.join("app/Config/Routes"), &mut route_files, 0);
    for f in route_files {
        if let Some(src) = read(&f) {
            routes(&SourceFile::new(&f, &src), &mut d.routes);
        }
    }
    controllers(root, read, &mut d.handlers);
    views(root, &mut d.views);
    languages(root, read, &mut d.trans);
    configs(root, &mut d.config);
}

fn routes(file: &SourceFile, out: &mut Vec<Entry>) {
    struct V<'a, 'b> {
        file: &'a SourceFile<'b>,
        out: &'a mut Vec<Entry>,
    }
    impl<'ast> Visitor<'ast> for V<'_, '_> {
        fn visit_expr(&mut self, e: &'ast Expr) {
            if let ExprKind::MethodCall { name: MemberName::Ident(m), args, .. } = &e.kind {
                let method = m.text.to_ascii_lowercase();
                let at = if method == "match" { 1 } else { 0 };
                if ROUTE_METHODS.contains(&method.as_str()) || method == "match" {
                    let path = args.get(at).and_then(|a| a.value.string_value(self.file.src)).unwrap_or("");
                    let handler = args.get(at + 1).and_then(|a| a.value.string_value(self.file.src)).unwrap_or("");
                    if let Some(ExprKind::Array(items)) = args.get(at + 2).map(|a| &a.value.kind) {
                        for it in items {
                            let key = it.key.as_ref().and_then(|k| k.string_value(self.file.src));
                            if key != Some("as") {
                                continue;
                            }
                            if let Some(name) = it.value.string_value(self.file.src) {
                                let sp = it.value.span;
                                let span = php_syntax::Span::new(sp.start as usize + 1, sp.end as usize - 1);
                                let detail =
                                    format!("{} /{} → {handler}", method.to_uppercase(), path.trim_start_matches('/'));
                                self.out.push(self.file.entry(name.to_string(), span, detail));
                            }
                        }
                    }
                }
            }
            walk_expr(self, e);
        }
    }
    let parse = php_syntax::parse(file.src);
    walk_stmts(&mut V { file, out }, &parse.stmts);
}

/// `Sub\Class::method` for every public method of the controllers under `app/Controllers`.
fn controllers(root: &Path, read: &Reader, out: &mut Vec<Entry>) {
    let dir = root.join("app/Controllers");
    let mut fs = Vec::new();
    files(&dir, &mut fs, 0);
    for f in fs.iter().filter(|f| f.extension().is_some_and(|e| e == "php")) {
        let Some(src) = read(f) else { continue };
        let parse = php_syntax::parse(&src);
        let file = SourceFile::new(f, &src);
        let rel_dir =
            f.parent().and_then(|p| p.strip_prefix(&dir).ok()).map(|p| p.to_string_lossy().replace('/', "\\"));
        let prefix = rel_dir.filter(|r| !r.is_empty()).map(|r| format!("{r}\\")).unwrap_or_default();
        for c in classes(&parse.stmts) {
            let (Some(cname), false) = (&c.name, c.modifiers.is_abstract) else { continue };
            for m in &c.members {
                let Member::Method { decl, modifiers } = m else { continue };
                let Some(mname) = &decl.name else { continue };
                if modifiers.vis.is_some_and(|v| v != Visibility::Public)
                    || modifiers.is_static
                    || mname.text.starts_with('_')
                    || mname.text.eq_ignore_ascii_case("initController")
                {
                    continue;
                }
                let key = format!("{prefix}{}::{}", cname.text, mname.text);
                let shown = f.strip_prefix(root).unwrap_or(f).display().to_string();
                out.push(file.entry(key, mname.span, shown));
            }
        }
    }
}

fn classes(stmts: &[Stmt]) -> Vec<&php_syntax::ClassDecl> {
    let mut out = Vec::new();
    for s in stmts {
        match &s.kind {
            StmtKind::Class(c) => out.push(&**c),
            StmtKind::Namespace { body: Some(b), .. } => out.extend(classes(b)),
            _ => {}
        }
    }
    out
}

fn views(root: &Path, out: &mut Vec<Entry>) {
    let dir = root.join("app/Views");
    let mut fs = Vec::new();
    files(&dir, &mut fs, 0);
    for p in fs {
        let Ok(rel) = p.strip_prefix(&dir) else { continue };
        let rel = rel.to_string_lossy().replace('\\', "/");
        if let Some(key) = rel.strip_suffix(".php") {
            let shown = p.strip_prefix(root).unwrap_or(&p).display().to_string();
            out.push(file_entry(&p, key.to_string(), shown));
        }
    }
}

/// `<dir>/<locale>/File.php` for the application, the framework and installed packages.
fn languages(root: &Path, read: &Reader, out: &mut Vec<Entry>) {
    let mut dirs = vec![root.join("app/Language"), root.join("vendor/codeigniter4/framework/system/Language")];
    for vendor in std::fs::read_dir(root.join("vendor")).into_iter().flatten().flatten() {
        for pkg in std::fs::read_dir(vendor.path()).into_iter().flatten().flatten() {
            dirs.push(pkg.path().join("src/Language"));
        }
    }
    for dir in dirs {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut locales: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        locales.sort();
        for l in locales {
            let locale = l.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
            trans_dir(&l, &locale, read, out);
        }
    }
}

fn configs(root: &Path, out: &mut Vec<Entry>) {
    let dir = root.join("app/Config");
    let Ok(rd) = std::fs::read_dir(&dir) else { return };
    let mut fs: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "php")).collect();
    fs.sort();
    for p in fs {
        let Some(stem) = p.file_stem().map(|s| s.to_string_lossy().into_owned()) else { continue };
        if stem == "Routes" {
            continue;
        }
        let shown = p.strip_prefix(root).unwrap_or(&p).display().to_string();
        out.push(file_entry(&p, stem, shown));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_names() {
        let src = r#"<?php
$routes->get('/', 'Home::index', ['as' => 'home']);
$routes->group('admin', static function ($routes) {
    $routes->post('users/(:num)', 'Admin\Users::update/$1', ['as' => 'admin.users.update']);
});
$routes->match(['get', 'post'], 'login', 'Auth::login', ['as' => 'login']);
$routes->get('about', 'Pages::about');
"#;
        let file = SourceFile::new(Path::new("/p/app/Config/Routes.php"), src);
        let mut out = Vec::new();
        routes(&file, &mut out);
        let keys: Vec<&str> = out.iter().map(|e| e.key.as_str()).collect();
        assert_eq!(keys, ["home", "admin.users.update", "login"]);
    }
}
