//! Variables a Blade view receives: the data passed where the view is rendered, with the types of
//! the passed values, plus the variables Laravel shares with every view.
//!
//! Render calls recognised (in the project's own PHP files that mention `view`):
//! `view('name', [...])`, `View::make('name', [...])`, `response()->view('name', [...])`,
//! `Route::view('/uri', 'name', [...])`, the data as an array literal or `compact('a', 'b')`,
//! and `->with('key', $value)` / `->with([...])` chained on any of them.

use php_index::{analyze_with, name_hash, types::Types, Index, RefTarget};
use php_syntax::walk::{walk_expr, walk_stmts, Visitor};
use php_syntax::{Arg, ClassRef, Expr, ExprKind, MemberName, Span};
use std::path::Path;

/// Dotted view name of a template under `<root>/resources/views`.
pub fn view_name(root: &Path, file: &Path) -> Option<String> {
    let rel = file.strip_prefix(root.join("resources").join("views")).ok()?;
    let s = rel.to_string_lossy().replace('\\', "/");
    let stem = s.strip_suffix(".blade.php")?;
    Some(stem.replace('/', "."))
}

/// CodeIgniter view name of a file under `<root>/app/Views` (`users/index`).
pub fn ci_view_name(root: &Path, file: &Path) -> Option<String> {
    let rel = file.strip_prefix(root.join("app").join("Views")).ok()?;
    let s = rel.to_string_lossy().replace('\\', "/");
    Some(s.strip_suffix(".php")?.to_string())
}

/// Could `uri` be a view whose variables come from elsewhere (a Blade template or a CodeIgniter view)?
pub fn maybe_view(uri: &str) -> bool {
    php_syntax::blade::is_blade(uri) || uri.contains("/app/Views/")
}

/// Variables every view (or every component view) has.
pub fn shared(component: bool) -> Vec<(String, Types)> {
    let mut v = vec![("errors".to_string(), vec!["Illuminate\\Support\\ViewErrorBag".to_string()])];
    if component {
        v.push(("attributes".to_string(), vec!["Illuminate\\View\\ComponentAttributeBag".to_string()]));
        v.push(("slot".to_string(), vec!["Illuminate\\Support\\HtmlString".to_string()]));
    }
    v
}

/// What one render call passes.
enum Data<'a> {
    /// `'key' => value` pairs.
    Pairs(Vec<(String, &'a Expr)>),
    /// `compact('a', 'b')` at the call's offset.
    Compact(Vec<String>, u32),
}

struct Finder<'a, 'ast> {
    src: &'a str,
    name: &'a str,
    found: Vec<Data<'ast>>,
}

fn str_lit(src: &str, e: &Expr) -> Option<String> {
    e.string_value(src).map(str::to_string)
}

impl<'ast> Finder<'_, 'ast> {
    /// The data argument of a render call (array literal or `compact(...)`).
    fn data(&mut self, e: &'ast Expr) {
        match &e.kind {
            ExprKind::Array(items) => {
                let pairs = items
                    .iter()
                    .filter_map(|i| Some((str_lit(self.src, i.key.as_ref()?)?, &i.value)))
                    .collect::<Vec<_>>();
                self.found.push(Data::Pairs(pairs));
            }
            ExprKind::Call { callee, args } if matches!(&callee.kind, ExprKind::Name(n) if n.text.eq_ignore_ascii_case("compact")) =>
            {
                let names = args.iter().filter_map(|a| str_lit(self.src, &a.value)).collect();
                self.found.push(Data::Compact(names, e.span.start));
            }
            _ => {}
        }
    }

    /// Is `e` a render call of our view? Then record its data argument.
    fn render_call(&mut self, e: &'ast Expr) -> bool {
        let (args, name_at): (&'ast [Arg], usize) = match &e.kind {
            ExprKind::Call { callee, args } if matches!(&callee.kind, ExprKind::Name(n) if n.text.eq_ignore_ascii_case("view")) => {
                (args, 0)
            }
            ExprKind::StaticCall { class: ClassRef::Name(c), name: MemberName::Ident(m), args }
                if c.text
                    .trim_start_matches('\\')
                    .rsplit('\\')
                    .next()
                    .is_some_and(|s| s.eq_ignore_ascii_case("View"))
                    && m.text.eq_ignore_ascii_case("make") =>
            {
                (args, 0)
            }
            ExprKind::StaticCall { class: ClassRef::Name(c), name: MemberName::Ident(m), args }
                if c.text
                    .trim_start_matches('\\')
                    .rsplit('\\')
                    .next()
                    .is_some_and(|s| s.eq_ignore_ascii_case("Route"))
                    && m.text.eq_ignore_ascii_case("view") =>
            {
                (args, 1)
            }
            ExprKind::MethodCall { name: MemberName::Ident(m), args, .. } if m.text.eq_ignore_ascii_case("view") => {
                (args, 0)
            }
            _ => return false,
        };
        if args.get(name_at).and_then(|a| str_lit(self.src, &a.value)).as_deref() != Some(self.name) {
            return false;
        }
        if let Some(d) = args.get(name_at + 1) {
            self.data(&d.value);
        }
        true
    }

    /// `->with(...)` chains ending in a render call of our view.
    fn with_chain(&mut self, e: &'ast Expr) -> bool {
        match &e.kind {
            ExprKind::MethodCall { obj, name: MemberName::Ident(m), args, .. }
                if m.text.eq_ignore_ascii_case("with") =>
            {
                if !self.with_chain(obj) {
                    return false;
                }
                match args.as_slice() {
                    [key, value] => {
                        if let Some(k) = str_lit(self.src, &key.value) {
                            self.found.push(Data::Pairs(vec![(k, &value.value)]));
                        }
                    }
                    [data] => self.data(&data.value),
                    _ => {}
                }
                true
            }
            _ => self.render_call(e),
        }
    }
}

impl<'ast> Visitor<'ast> for Finder<'_, 'ast> {
    fn visit_expr(&mut self, e: &'ast Expr) {
        if self.with_chain(e) {
            // the inner calls were handled as part of the chain
            if let ExprKind::MethodCall { args, .. } | ExprKind::Call { args, .. } | ExprKind::StaticCall { args, .. } =
                &e.kind
            {
                for a in args {
                    walk_expr(self, &a.value);
                }
            }
            return;
        }
        walk_expr(self, e);
    }
}

/// Variables passed to view `name` from the project files under `root_uri` (a `file://` URI
/// prefix), with the union of the types passed for each.
pub fn collect(
    index: &Index,
    root_uri: &str,
    name: &str,
    read: &dyn Fn(&str) -> Option<String>,
) -> Vec<(String, Types)> {
    let quoted = [format!("'{name}'"), format!("\"{name}\"")];
    let mut out: Vec<(String, Types)> = Vec::new();
    for id in index.files_with_name(name_hash("view")) {
        let Some(file) = index.file(id) else { continue };
        let uri = &*file.uri;
        if !uri.starts_with(root_uri) || index.is_library(uri) || php_syntax::blade::is_blade(uri) {
            continue;
        }
        let Some(src) = read(uri) else { continue };
        if !quoted.iter().any(|q| src.contains(q.as_str())) {
            continue;
        }
        let parse = php_syntax::parse(&src);
        let mut f = Finder { src: &src, name, found: Vec::new() };
        walk_stmts(&mut f, &parse.stmts);
        if f.found.is_empty() {
            continue;
        }
        let want: Vec<Span> = f
            .found
            .iter()
            .flat_map(|d| match d {
                Data::Pairs(p) => p.iter().map(|(_, e)| e.span).collect::<Vec<_>>(),
                Data::Compact(..) => Vec::new(),
            })
            .collect();
        let analysis = analyze_with(index, &src, &parse.stmts, &[], &want);
        for d in &f.found {
            match d {
                Data::Pairs(pairs) => {
                    for (k, e) in pairs {
                        let t = analysis.expr_types.iter().find(|(s, _)| *s == e.span).map(|(_, t)| t.clone());
                        add(&mut out, k, t.unwrap_or_default());
                    }
                }
                Data::Compact(names, at) => {
                    for n in names {
                        // the variable as last seen before the call
                        let t = analysis
                            .refs
                            .iter()
                            .filter(|r| r.span.start < *at)
                            .filter_map(|r| match &r.target {
                                RefTarget::Variable { name: v, classes, .. } if v == n => Some(classes.clone()),
                                _ => None,
                            })
                            .next_back();
                        add(&mut out, n, t.unwrap_or_default());
                    }
                }
            }
        }
    }
    out
}

fn add(out: &mut Vec<(String, Types)>, name: &str, types: Types) {
    match out.iter_mut().find(|(n, _)| n == name) {
        Some((_, t)) => {
            for c in types {
                if !t.contains(&c) {
                    t.push(c);
                }
            }
        }
        None => out.push((name.to_string(), types)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_views_by_path() {
        let root = Path::new("/app");
        assert_eq!(
            view_name(root, Path::new("/app/resources/views/users/index.blade.php")).as_deref(),
            Some("users.index")
        );
        assert_eq!(view_name(root, Path::new("/app/resources/views/home.blade.php")).as_deref(), Some("home"));
        assert_eq!(view_name(root, Path::new("/app/app/Http/Kernel.php")), None);
    }
}
