//! Global class aliases of a Laravel application (`Route::`, `Str::`, `Form::` without a `use`):
//! the framework defaults, the `aliases` array of `config/app.php` and the aliases installed
//! packages declare for auto-discovery (`extra.laravel.aliases` in `vendor/composer/installed.json`).

use php_syntax::walk::{walk_expr, walk_stmts, Visitor};
use php_syntax::{Expr, ExprKind};
use std::path::Path;

/// Laravel's `Facade::defaultAliases()`.
fn defaults() -> Vec<(String, String)> {
    const FACADES: &[&str] = &[
        "App",
        "Artisan",
        "Auth",
        "Blade",
        "Broadcast",
        "Bus",
        "Cache",
        "Concurrency",
        "Config",
        "Context",
        "Cookie",
        "Crypt",
        "Date",
        "DB",
        "Event",
        "File",
        "Gate",
        "Hash",
        "Http",
        "Lang",
        "Log",
        "Mail",
        "Notification",
        "Password",
        "Process",
        "Queue",
        "RateLimiter",
        "Redirect",
        "Request",
        "Response",
        "Route",
        "Schedule",
        "Schema",
        "Session",
        "Storage",
        "URL",
        "Validator",
        "View",
        "Vite",
    ];
    let mut out: Vec<(String, String)> =
        FACADES.iter().map(|f| (f.to_string(), format!("Illuminate\\Support\\Facades\\{f}"))).collect();
    for (alias, target) in [
        ("Arr", "Illuminate\\Support\\Arr"),
        ("Str", "Illuminate\\Support\\Str"),
        ("Js", "Illuminate\\Support\\Js"),
        ("Number", "Illuminate\\Support\\Number"),
        ("Uri", "Illuminate\\Support\\Uri"),
        ("Eloquent", "Illuminate\\Database\\Eloquent\\Model"),
    ] {
        out.push((alias.to_string(), target.to_string()));
    }
    out
}

/// Every alias of the application at `root`; later sources override earlier ones.
pub fn aliases(root: &Path) -> Vec<(String, String)> {
    let mut out = defaults();
    let mut add = |alias: String, target: String| {
        out.retain(|(a, _)| !a.eq_ignore_ascii_case(&alias));
        out.push((alias, target));
    };
    if let Ok(json) = std::fs::read_to_string(root.join("vendor/composer/installed.json")) {
        for (a, t) in package_aliases(&json) {
            add(a, t);
        }
    }
    if let Ok(src) = std::fs::read_to_string(root.join("config/app.php")) {
        for (a, t) in config_aliases(&src) {
            add(a, t);
        }
    }
    out
}

/// `extra.laravel.aliases` of every package in `installed.json` (Composer 1 and 2 layouts).
fn package_aliases(json: &str) -> Vec<(String, String)> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return Vec::new() };
    let packages = v.get("packages").unwrap_or(&v);
    let mut out = Vec::new();
    for p in packages.as_array().into_iter().flatten() {
        let Some(map) = p.pointer("/extra/laravel/aliases").and_then(|a| a.as_object()) else { continue };
        for (alias, target) in map {
            if let Some(t) = target.as_str() {
                out.push((alias.clone(), t.trim_start_matches('\\').to_string()));
            }
        }
    }
    out
}

/// `'aliases' => [...]` in `config/app.php`, also inside `Facade::defaultAliases()->merge([...])`.
fn config_aliases(src: &str) -> Vec<(String, String)> {
    struct Finder<'s> {
        src: &'s str,
        inside: bool,
        out: Vec<(String, String)>,
    }
    impl<'ast> Visitor<'ast> for Finder<'_> {
        fn visit_expr(&mut self, e: &'ast Expr) {
            if let ExprKind::Array(items) = &e.kind {
                for item in items {
                    let key = item.key.as_ref().and_then(|k| k.string_value(self.src));
                    if !self.inside {
                        if key == Some("aliases") {
                            self.inside = true;
                            self.visit_expr(&item.value);
                            self.inside = false;
                        } else {
                            self.visit_expr(&item.value);
                        }
                        continue;
                    }
                    let target = match &item.value.kind {
                        ExprKind::ClassConst { class: php_syntax::ClassRef::Name(n), name } if name.text == "class" => {
                            Some(n.text.trim_start_matches('\\').to_string())
                        }
                        _ => item.value.string_value(self.src).map(|s| s.trim_start_matches('\\').to_string()),
                    };
                    if let (Some(k), Some(t)) = (key, target) {
                        self.out.push((k.to_string(), t));
                    }
                }
                return;
            }
            walk_expr(self, e);
        }
    }
    let parse = php_syntax::parse(src);
    let mut f = Finder { src, inside: false, out: Vec::new() };
    walk_stmts(&mut f, &parse.stmts);
    f.out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_config_and_packages() {
        let config = r#"<?php
return [
    'name' => 'x',
    'aliases' => Illuminate\Support\Facades\Facade::defaultAliases()->merge([
        'Form' => Collective\Html\FormFacade::class,
        'Helper' => '\App\Support\Helper',
    ])->toArray(),
];"#;
        assert_eq!(
            config_aliases(config),
            [
                ("Form".to_string(), "Collective\\Html\\FormFacade".to_string()),
                ("Helper".to_string(), "App\\Support\\Helper".to_string())
            ]
        );
        let json = r#"{"packages": [{"name": "a/b", "extra": {"laravel": {"aliases": {"Image": "Intervention\\Image\\Facades\\Image"}}}}]}"#;
        assert_eq!(package_aliases(json), [("Image".to_string(), "Intervention\\Image\\Facades\\Image".to_string())]);
    }
}
