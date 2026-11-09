//! Symfony: route names and Twig template names.
//!
//! Routes come from `#[Route]` attributes and `@Route` annotations on the controllers under `src/`
//! (with the class-level name prefix, and Symfony's generated name when a route has none) and from
//! the YAML files `config/routes.yaml` / `config/routes/*.yaml`. Templates are the files under
//! `templates/`, named by their path (`blog/index.html.twig`).

use super::{file_entry, files, Entry, LaravelData, Reader, SourceFile};
use php_syntax::{Arg, ClassDecl, ClassRef, Expr, ExprKind, Member, Stmt, StmtKind};
use std::path::Path;

pub(super) fn build(root: &Path, read: &Reader, d: &mut LaravelData) {
    let mut fs = Vec::new();
    files(&root.join("src"), &mut fs, 0);
    for f in fs.iter().filter(|f| f.extension().is_some_and(|e| e == "php")) {
        let Some(src) = read(f) else { continue };
        if !src.contains("Route") {
            continue;
        }
        let parse = php_syntax::parse(&src);
        let file = SourceFile::new(f, &src);
        controller_routes(&file, &parse.stmts, "", &mut d.routes);
    }
    for f in [root.join("config/routes.yaml")].into_iter().chain(yaml_files(&root.join("config/routes"))) {
        if let Some(src) = read(&f) {
            yaml_routes(&SourceFile::new(&f, &src), &mut d.routes);
        }
    }
    let dir = root.join("templates");
    let mut ts = Vec::new();
    files(&dir, &mut ts, 0);
    for t in ts {
        let Ok(rel) = t.strip_prefix(&dir) else { continue };
        let key = rel.to_string_lossy().replace('\\', "/");
        if key.ends_with(".twig") {
            let shown = t.strip_prefix(root).unwrap_or(&t).display().to_string();
            d.views.push(file_entry(&t, key, shown));
        }
    }
}

fn yaml_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut fs = Vec::new();
    files(dir, &mut fs, 0);
    fs.retain(|f| f.extension().is_some_and(|e| e == "yaml" || e == "yml"));
    fs
}

fn controller_routes(file: &SourceFile, stmts: &[Stmt], ns: &str, out: &mut Vec<Entry>) {
    let mut ns = ns.to_string();
    for s in stmts {
        match &s.kind {
            StmtKind::Namespace { name, body } => {
                let n = name.as_ref().map(|n| n.text.clone()).unwrap_or_default();
                match body {
                    Some(b) => controller_routes(file, b, &n, out),
                    None => ns = n,
                }
            }
            StmtKind::Class(c) => class_routes(file, c, &ns, s.doc, out),
            _ => {}
        }
    }
}

/// What a route attribute or annotation says: its path and name, where the name is written.
struct RouteDecl {
    path: String,
    name: Option<(String, php_syntax::Span)>,
}

fn class_routes(file: &SourceFile, c: &ClassDecl, ns: &str, stmt_doc: Option<php_syntax::Span>, out: &mut Vec<Entry>) {
    let Some(class_name) = &c.name else { return };
    let fqn = if ns.is_empty() { class_name.text.clone() } else { format!("{ns}\\{}", class_name.text) };
    let doc = c.doc.or(stmt_doc);
    let class_route = routes_of(file.src, &c.attrs, doc).into_iter().next();
    let prefix_name = class_route.as_ref().and_then(|r| r.name.as_ref()).map(|(n, _)| n.clone()).unwrap_or_default();
    let prefix_path = class_route.map(|r| r.path).unwrap_or_default();
    for m in &c.members {
        let Member::Method { decl, .. } = m else { continue };
        let Some(method) = &decl.name else { continue };
        // a method may have several routes
        for route in routes_of(file.src, &decl.attrs, decl.doc) {
            let detail = format!("{prefix_path}{} → {}::{}", route.path, class_name.text, method.text);
            let (key, span) = match route.name {
                Some((n, sp)) => (format!("{prefix_name}{n}"), sp),
                None => (format!("{prefix_name}{}", default_name(&fqn, &method.text)), method.span),
            };
            out.push(file.entry(key, span, detail));
        }
    }
}

/// Symfony's name for a route without one (`AttributeClassLoader::getDefaultRouteName`):
/// `App\Controller\BlogController::index` → `app_blog_index`.
fn default_name(class_fqn: &str, method: &str) -> String {
    let mut name = format!("{}_{}", class_fqn.replace('\\', "_"), method).to_lowercase();
    name = name.replace("bundle_", "_").replace("controller_", "_");
    if method.ends_with("Action") || method.ends_with("_action") {
        if let Some(stripped) = name.strip_suffix("action") {
            name = stripped.to_string();
        }
    }
    while name.contains("__") {
        name = name.replace("__", "_");
    }
    name
}

/// Every route of a declaration: `#[Route]` attributes, then `@Route` annotations of its docblock.
fn routes_of(src: &str, attrs: &[Expr], doc: Option<php_syntax::Span>) -> Vec<RouteDecl> {
    let mut out = route_attrs(src, attrs);
    if let Some(d) = doc {
        out.extend(route_annotations(src, d));
    }
    out
}

/// `#[Route('/path', name: 'x')]` (also `Symfony\…\Route`, `path:` named).
fn route_attrs(src: &str, attrs: &[Expr]) -> Vec<RouteDecl> {
    attrs
        .iter()
        .filter_map(|a| {
            let ExprKind::New { class: ClassRef::Name(n), args } = &a.kind else { return None };
            if n.text.rsplit('\\').next() != Some("Route") {
                return None;
            }
            Some(route_args(src, args))
        })
        .collect()
}

fn route_args(src: &str, args: &[Arg]) -> RouteDecl {
    let mut path = String::new();
    let mut name = None;
    for (i, a) in args.iter().enumerate() {
        let arg_name = a.name.as_ref().map(|n| n.text.as_str());
        let value = a.value.string_value(src);
        match (arg_name, i) {
            (Some("path"), _) | (None, 0) => path = value.unwrap_or_default().to_string(),
            (Some("name"), _) => {
                if let Some(v) = value {
                    let sp = a.value.span;
                    name = Some((v.to_string(), php_syntax::Span::new(sp.start as usize + 1, sp.end as usize - 1)));
                }
            }
            _ => {}
        }
    }
    RouteDecl { path, name }
}

/// `@Route("/path", name="x")` annotations in a docblock (`@Route (` with a space too).
fn route_annotations(src: &str, doc: php_syntax::Span) -> Vec<RouteDecl> {
    let text = doc.text(src);
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = text[from..].find("@Route") {
        let tag = from + i;
        from = tag + 6;
        let rest = &text[from..];
        let open = rest.len() - rest.trim_start().len();
        if !rest[open..].starts_with('(') || text[from..].starts_with(|c: char| c.is_alphanumeric()) {
            continue;
        }
        let body_start = from + open + 1;
        let body = &text[body_start..];
        let body = &body[..body.find(')').unwrap_or(body.len())];
        let path = body.trim_start().strip_prefix('"').and_then(|r| r.split('"').next()).unwrap_or("").to_string();
        let name = body.find("name=\"").map(|j| {
            let start = j + 6;
            let len = body[start..].find('"').unwrap_or(0);
            let abs = doc.start as usize + body_start + start;
            (body[start..start + len].to_string(), php_syntax::Span::new(abs, abs + len))
        });
        out.push(RouteDecl { path, name });
    }
    out
}

/// Top-level keys of a routes YAML file whose block has a `path:` (imports have `resource:`).
fn yaml_routes(file: &SourceFile, out: &mut Vec<Entry>) {
    let mut off = 0;
    let mut current: Option<(String, usize)> = None;
    let mut push = |cur: &mut Option<(String, usize)>, path: &str| {
        if let Some((name, at)) = cur.take() {
            out.push(file.entry(name.clone(), php_syntax::Span::new(at, at + name.len()), path.to_string()));
        }
    };
    for line in file.src.split_inclusive('\n') {
        let here = off;
        off += line.len();
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        if !line.starts_with([' ', '\t']) {
            current = None;
            let key = line.trim_end().strip_suffix(':').map(str::trim);
            if let Some(k) = key {
                if !k.is_empty()
                    && !k.starts_with("when@")
                    && k.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c))
                {
                    current = Some((k.to_string(), here));
                }
            }
        } else if let Some(p) = line.trim().strip_prefix("path:") {
            push(&mut current, p.trim().trim_matches(['\'', '"']));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_route_names() {
        assert_eq!(default_name("App\\Controller\\BlogController", "index"), "app_blog_index");
        assert_eq!(default_name("App\\Controller\\Admin\\UserController", "editAction"), "app_admin_user_edit");
    }

    #[test]
    fn routes_from_attributes_annotations_and_yaml() {
        let src = r#"<?php
namespace App\Controller;
use Symfony\Component\Routing\Attribute\Route;
#[Route('/blog', name: 'blog_')]
class BlogController {
    #[Route('/', name: 'index')]
    public function index() {}
    #[Route('/{slug}')]
    public function show() {}
    /**
     * @Route("/old", name="legacy")
     * @Route ("/older", name="legacy_two")
     */
    public function old() {}
}
"#;
        let file = SourceFile::new(Path::new("/p/src/Controller/BlogController.php"), src);
        let parse = php_syntax::parse(src);
        let mut out = Vec::new();
        controller_routes(&file, &parse.stmts, "", &mut out);
        let keys: Vec<&str> = out.iter().map(|e| e.key.as_str()).collect();
        assert_eq!(keys, ["blog_index", "blog_app_blog_show", "blog_legacy", "blog_legacy_two"]);

        let yaml = "home:\n    path: /\n    controller: App\\Controller\\Home::index\ncontrollers:\n    resource: ../src/Controller/\n";
        let file = SourceFile::new(Path::new("/p/config/routes.yaml"), yaml);
        let mut out = Vec::new();
        yaml_routes(&file, &mut out);
        assert_eq!(out.iter().map(|e| e.key.as_str()).collect::<Vec<_>>(), ["home"]);
    }
}
