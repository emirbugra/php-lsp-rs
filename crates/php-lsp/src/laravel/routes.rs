//! Route names from `routes/*.php`, `bootstrap/app.php`, service providers and modules.
//!
//! A route is declared by a chain of calls (`Route::prefix('admin')->name('admin.')->group(...)`,
//! `Route::get('/', ...)->name('home')`). Each chain is read link by link; `group` closures and
//! group files are walked with the group's name / URI prefix.

use super::{arg, files, lit, normalize, one_line, Entry, Reader, SourceFile};
use php_syntax::walk::{walk_expr, Visitor};
use php_syntax::{Arg, ArrayItem, ClassRef, Expr, ExprKind, MemberName, Span};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Name and URI prefix of the enclosing route groups.
#[derive(Clone, Default)]
pub(super) struct RouteGroup {
    name: String,
    uri: String,
}

fn join_uri(a: &str, b: &str) -> String {
    let a = a.trim_matches('/');
    let b = b.trim_matches('/');
    match (a.is_empty(), b.is_empty()) {
        (true, true) => "/".into(),
        (true, false) => format!("/{b}"),
        (false, true) => format!("/{a}"),
        (false, false) => format!("/{a}/{b}"),
    }
}

/// One call of a chain: lowercase method name and its arguments.
struct Link<'a> {
    name: String,
    args: &'a [Arg],
}

/// `Route::get(...)->name(...)->middleware(...)` -> [get, name, middleware]. The chain must start at
/// the `Route` facade or a `$router` variable.
fn links_of(e: &Expr) -> Option<Vec<Link<'_>>> {
    match &e.kind {
        ExprKind::MethodCall { obj, name: MemberName::Ident(id), args, .. } => {
            let mut v = match &obj.kind {
                ExprKind::Var(n) if n.trim_start_matches('$') == "router" => Vec::new(),
                _ => links_of(obj)?,
            };
            v.push(Link { name: id.text.to_ascii_lowercase(), args });
            Some(v)
        }
        ExprKind::StaticCall { class: ClassRef::Name(n), name: MemberName::Ident(id), args } => {
            let last = n.text.rsplit('\\').next().unwrap_or(&n.text);
            if last.eq_ignore_ascii_case("Route") {
                Some(vec![Link { name: id.text.to_ascii_lowercase(), args }])
            } else {
                None
            }
        }
        _ => None,
    }
}

const VERBS: &[&str] = &["get", "post", "put", "patch", "delete", "options", "any"];

/// What one chain declares, collected link by link.
#[derive(Default)]
struct RouteChain<'e> {
    /// (verbs, uri, action) of `get('/uri', action)` and friends.
    route: Option<(String, String, Option<&'e Expr>)>,
    /// `->name('x')` after the route / resource.
    name: Option<(String, Span)>,
    /// (api only, resource name, span) of `resource('photos', ...)`.
    resource: Option<(bool, String, Span)>,
    options: ResourceOptions,
}

/// `->only([...])`, `->except([...])`, `->names(...)` of a resource.
#[derive(Default)]
struct ResourceOptions {
    only: Vec<String>,
    except: Vec<String>,
    /// `->names('base')`
    names_base: Option<(String, Span)>,
    /// `->names(['index' => 'x', ...])`: (action, name, span)
    names_map: Vec<(String, String, Span)>,
}

pub(super) struct RouteWalker<'a, 'r> {
    pub(super) root: &'a Path,
    pub(super) path: &'a Path,
    pub(super) source: SourceFile<'a>,
    pub(super) group: RouteGroup,
    pub(super) out: &'r mut Vec<Entry>,
    /// Route files loaded through `->group(base_path('routes/x.php'))`, with the group's prefixes.
    pub(super) file_groups: &'r mut HashMap<PathBuf, RouteGroup>,
}

impl RouteWalker<'_, '_> {
    /// `base_path('routes/x.php')`, `__DIR__ . '/x.php'` or `'routes/x.php'`.
    fn group_file(&self, e: &Expr) -> Option<PathBuf> {
        let src = self.source.src;
        match &e.kind {
            ExprKind::Call { callee, args } => {
                let ExprKind::Name(n) = &callee.kind else { return None };
                if !n.text.trim_start_matches('\\').eq_ignore_ascii_case("base_path") {
                    return None;
                }
                Some(self.root.join(lit(src, arg(args, 0)?)?))
            }
            ExprKind::Binary { op: ".", lhs, rhs } => {
                let ExprKind::Name(n) = &lhs.kind else { return None };
                if n.text != "__DIR__" {
                    return None;
                }
                let rel = lit(src, rhs)?;
                Some(self.path.parent()?.join(rel.trim_start_matches('/')))
            }
            ExprKind::Lit => Some(self.root.join(lit(src, e)?)),
            _ => None,
        }
        .map(|p| normalize(&p))
    }

    /// `Route::group(['as' => 'x.', 'prefix' => 'y'], ...)`
    fn group_attrs(&self, items: &[ArrayItem], group: &mut RouteGroup) {
        let src = self.source.src;
        for it in items {
            let (Some(k), Some(v)) = (it.key.as_ref().and_then(|k| lit(src, k)), lit(src, &it.value)) else {
                continue;
            };
            match k.as_str() {
                "as" => group.name.push_str(&v),
                "prefix" => group.uri = join_uri(&group.uri, &v),
                _ => {}
            }
        }
    }

    /// laravel/ui `Auth::routes(['register' => false, ...])`
    fn auth_routes(&mut self, span: Span, args: &[Arg]) {
        let src = self.source.src;
        let off = |opt: &str| {
            let Some(ExprKind::Array(items)) = arg(args, 0).map(|a| &a.kind) else { return false };
            items.iter().any(|i| {
                i.key.as_ref().and_then(|k| lit(src, k)).as_deref() == Some(opt)
                    && i.value.span.text(src).eq_ignore_ascii_case("false")
            })
        };
        let mut names = vec!["login", "logout"];
        if !off("register") {
            names.push("register");
        }
        if !off("reset") {
            names.extend(["password.request", "password.email", "password.reset", "password.update"]);
        }
        if !off("confirm") {
            names.push("password.confirm");
        }
        if !off("verify") {
            names.extend(["verification.notice", "verification.verify", "verification.resend"]);
        }
        for n in names {
            let e = self.source.entry(format!("{}{n}", self.group.name), span, "Auth::routes()".into());
            self.out.push(e);
        }
    }

    /// `['a', 'b']` or a single `'a'`.
    fn string_list(&self, e: Option<&Expr>) -> Vec<String> {
        match e.map(|e| &e.kind) {
            Some(ExprKind::Array(items)) => items.iter().filter_map(|i| lit(self.source.src, &i.value)).collect(),
            Some(_) => e.and_then(|e| lit(self.source.src, e)).into_iter().collect(),
            None => Vec::new(),
        }
    }

    /// Read one chain and record the route(s) it declares.
    fn process(&mut self, links: Vec<Link<'_>>) {
        let mut group = self.group.clone();
        let mut chain = RouteChain::default();
        for l in &links {
            self.apply_link(l, &mut chain, &mut group);
        }
        if let Some((verb, uri, action)) = chain.route {
            if let Some((n, span)) = chain.name {
                let action =
                    action.map(|a| format!(" → {}", one_line(a.span.text(self.source.src), 70))).unwrap_or_default();
                let detail = format!("{verb} {}{action}", join_uri(&group.uri, &uri));
                self.out.push(self.source.entry(format!("{}{n}", group.name), span, detail));
            }
        } else if let Some((api, res, span)) = chain.resource {
            self.push_resource(&group, api, &res, span, &chain.options);
        }
    }

    fn apply_link<'e>(&mut self, l: &Link<'e>, chain: &mut RouteChain<'e>, group: &mut RouteGroup) {
        let src = self.source.src;
        let a0 = arg(l.args, 0);
        let a0_str = || a0.and_then(|e| lit(src, e));
        match l.name.as_str() {
            "name" | "as" => {
                let Some(v) = a0_str() else { return };
                if chain.route.is_some() || chain.resource.is_some() {
                    chain.name = Some((v, a0.unwrap().span));
                } else {
                    group.name.push_str(&v);
                }
            }
            "prefix" => {
                if let Some(v) = a0_str() {
                    group.uri = join_uri(&group.uri, &v);
                }
            }
            "group" => self.walk_group(group, l.args),
            v if VERBS.contains(&v) && chain.route.is_none() => {
                if let Some(u) = a0_str() {
                    chain.route = Some((v.to_ascii_uppercase(), u, arg(l.args, 1)));
                }
            }
            "match" if chain.route.is_none() => {
                if let Some(u) = arg(l.args, 1).and_then(|e| lit(src, e)) {
                    let verbs = self.string_list(a0).join("|").to_ascii_uppercase();
                    chain.route = Some((verbs, u, arg(l.args, 2)));
                }
            }
            "view" | "redirect" | "permanentredirect" if chain.route.is_none() => {
                if let Some(u) = a0_str() {
                    chain.route = Some(("GET".into(), u, arg(l.args, 1)));
                }
            }
            "resource" | "apiresource" | "singleton" | "apisingleton" => {
                if let Some(r) = a0_str() {
                    chain.resource = Some((l.name.starts_with("api"), r, a0.unwrap().span));
                }
            }
            "resources" | "apiresources" => {
                let Some(ExprKind::Array(items)) = a0.map(|e| &e.kind) else { return };
                let api = l.name.starts_with("api");
                for it in items {
                    if let Some(k) = it.key.as_ref().and_then(|k| lit(src, k)) {
                        let span = it.key.as_ref().unwrap().span;
                        self.push_resource(group, api, &k, span, &ResourceOptions::default());
                    }
                }
            }
            "only" => chain.options.only = self.string_list(a0),
            "except" => chain.options.except = self.string_list(a0),
            "names" => match a0.map(|e| &e.kind) {
                Some(ExprKind::Array(items)) => {
                    for it in items {
                        if let (Some(k), Some(v)) = (it.key.as_ref().and_then(|k| lit(src, k)), lit(src, &it.value)) {
                            chain.options.names_map.push((k, v, it.value.span));
                        }
                    }
                }
                _ => {
                    if let Some(v) = a0_str() {
                        chain.options.names_base = Some((v, a0.unwrap().span));
                    }
                }
            },
            _ => {}
        }
    }

    /// `->group(...)`: attributes array, a closure walked with the group's prefixes, or a route file.
    fn walk_group(&mut self, group: &RouteGroup, args: &[Arg]) {
        let mut inner = group.clone();
        for a in args {
            match &a.value.kind {
                ExprKind::Array(items) => self.group_attrs(items, &mut inner),
                ExprKind::Closure(f) => {
                    let saved = std::mem::replace(&mut self.group, inner.clone());
                    if let Some(body) = &f.body {
                        php_syntax::walk::walk_stmts(self, body);
                    }
                    if let Some(b) = &f.arrow_body {
                        self.visit_expr(b);
                    }
                    self.group = saved;
                }
                _ => {
                    if let Some(p) = self.group_file(&a.value) {
                        self.file_groups.insert(p, inner.clone());
                    }
                }
            }
        }
    }

    fn push_resource(&mut self, group: &RouteGroup, api: bool, res: &str, span: Span, options: &ResourceOptions) {
        let actions: &[&str] = if api {
            &["index", "store", "show", "update", "destroy"]
        } else {
            &["index", "create", "store", "show", "edit", "update", "destroy"]
        };
        // Laravel: `a/b/photos` registers `photos` under the URI prefix `a/b`
        let res_name = res.rsplit('/').next().unwrap_or("").to_string();
        let (base, base_span) = options.names_base.clone().unwrap_or((res_name, span));
        for act in actions {
            if (!options.only.is_empty() && !options.only.iter().any(|o| o == act))
                || options.except.iter().any(|e| e == act)
            {
                continue;
            }
            let (key, at) = match options.names_map.iter().find(|(k, _, _)| k == act) {
                Some((_, v, s)) => (format!("{}{v}", group.name), *s),
                None => (format!("{}{}", group.name, format!("{base}.{act}").trim_matches('.')), base_span),
            };
            let detail = format!("resource {} ({act})", join_uri(&group.uri, &res.replace('.', "/")));
            self.out.push(self.source.entry(key, at, detail));
        }
    }
}

impl<'ast> Visitor<'ast> for RouteWalker<'_, '_> {
    fn visit_expr(&mut self, e: &'ast Expr) {
        if let Some(links) = links_of(e) {
            self.process(links);
            return;
        }
        match &e.kind {
            ExprKind::Include(inner) => {
                if let Some(p) = self.group_file(inner) {
                    if !self.group.name.is_empty() || !self.group.uri.is_empty() {
                        self.file_groups.insert(p, self.group.clone());
                    }
                }
            }
            ExprKind::StaticCall { class: ClassRef::Name(n), name: MemberName::Ident(id), args }
                if n.text.rsplit('\\').next().is_some_and(|c| c.eq_ignore_ascii_case("Auth"))
                    && id.text.eq_ignore_ascii_case("routes") =>
            {
                self.auth_routes(e.span, args);
            }
            _ => {}
        }
        walk_expr(self, e);
    }
}

pub(super) fn routes(root: &Path, read: &Reader, out: &mut Vec<Entry>) {
    // providers / bootstrap first: they may load route files inside named groups
    let mut order: Vec<PathBuf> = vec![root.join("bootstrap/app.php")];
    let mut providers = Vec::new();
    files(&root.join("app/Providers"), &mut providers, 0);
    order.extend(providers);
    let mut fs = Vec::new();
    files(&root.join("routes"), &mut fs, 0);
    // nwidart/laravel-modules style `Modules/<Name>/routes/*.php`
    for m in std::fs::read_dir(root.join("Modules")).into_iter().flatten().flatten() {
        files(&m.path().join("routes"), &mut fs, 0);
        files(&m.path().join("Routes"), &mut fs, 0);
    }
    order.extend(fs);
    let parsed: Vec<(PathBuf, String, php_syntax::Parse)> = order
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "php"))
        .filter_map(|p| {
            let src = read(&p)?;
            let parse = php_syntax::parse(&src);
            Some((p, src, parse))
        })
        .collect();

    // the first pass only learns which files are loaded inside groups (`->group(base_path(..))`,
    // `require __DIR__.'/admin.php'`), wherever they appear
    let mut groups: HashMap<PathBuf, RouteGroup> = HashMap::new();
    for pass in 0..2 {
        let mut scratch = Vec::new();
        let sink = if pass == 0 { &mut scratch } else { &mut *out };
        for (p, src, parse) in &parsed {
            let group = groups.get(&normalize(p)).cloned().unwrap_or_default();
            let mut w = RouteWalker {
                root,
                path: p,
                source: SourceFile::new(p, src),
                group,
                out: &mut *sink,
                file_groups: &mut groups,
            };
            php_syntax::walk::walk_stmts(&mut w, &parse.stmts);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::build;
    use std::path::Path;

    #[test]
    fn collects_route_names() {
        let root = std::env::temp_dir().join(format!("phplsp_routes_{}", std::process::id()));
        let w = |rel: &str, src: &str| {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, src).unwrap();
        };
        w("artisan", "");
        // `admin.php` sorts before `web.php`, yet still gets the prefix of the group requiring it
        w("routes/admin.php", "<?php Route::get('/', 'A@b')->name('home');");
        w(
            "routes/web.php",
            "<?php
Auth::routes(['register' => false]);
Route::name('admin.')->group(function () { require __DIR__ . '/admin.php'; });
Route::resources(['photos' => PhotoController::class]);
",
        );
        w("Modules/Blog/routes/web.php", "<?php Route::get('/blog', 'B@c')->name('blog.index');");
        w("config/app.php", "<?php return ['name' => 'x', 'providers' => [A::class], 'key' => env('K')];");
        let read = |p: &Path| std::fs::read_to_string(p).ok();
        let d = build(&root, &read);
        let names: Vec<&str> = d.routes.iter().map(|e| e.key.as_str()).collect();
        for n in ["login", "password.reset", "admin.home", "photos.edit", "blog.index"] {
            assert!(names.contains(&n), "{n} missing in {names:?}");
        }
        assert!(!names.contains(&"register") && !names.contains(&"home"), "{names:?}");
        let open = |k: &str| d.config.iter().find(|e| e.key == k).map(|e| e.open);
        assert_eq!(
            (open("app"), open("app.name"), open("app.providers"), open("app.key")),
            (Some(false), Some(false), Some(true), Some(true))
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
