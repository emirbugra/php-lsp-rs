//! Completion / go-to-definition / hover inside Laravel string arguments:
//! `route('…')`, `config('…')`, `view('…')`, `__('…')` / `trans('…')`, `env('…')` and their facade /
//! method forms (`Route::has`, `Config::get`, `View::make`, `Lang::get`, `->route()`, `->view()` …).
//!
//! - `strings.rs`: which string is under the cursor and which registry it refers to
//! - `diagnostics.rs`: names that do not exist (with a "did you mean" suggestion)

mod diagnostics;
mod strings;

pub use strings::string_at;

use crate::convert::*;
use crate::laravel::{self, Entry, Kind, LaravelData};
use crate::server::Server;
use diagnostics::unknown_keys;
use lsp_types::*;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use strings::{kind_at, StrLit};

/// How long collected Laravel data is reused before it is rebuilt from disk / open buffers.
const TTL: Duration = Duration::from_secs(2);

fn item_kind(k: Kind) -> CompletionItemKind {
    match k {
        Kind::Route => CompletionItemKind::REFERENCE,
        Kind::Config => CompletionItemKind::PROPERTY,
        Kind::Trans => CompletionItemKind::TEXT,
        Kind::View => CompletionItemKind::FILE,
        Kind::Env => CompletionItemKind::CONSTANT,
    }
}

fn kind_name(k: Kind) -> &'static str {
    match k {
        Kind::Route => "route",
        Kind::Config => "config",
        Kind::Trans => "translation",
        Kind::View => "view",
        Kind::Env => "env",
    }
}

fn label(k: Kind) -> &'static str {
    match k {
        Kind::Route => "Route",
        Kind::Config => "Config key",
        Kind::Trans => "Translation",
        Kind::View => "View",
        Kind::Env => "Env key",
    }
}

fn describe(e: &Entry) -> String {
    match &e.locale {
        Some(l) => format!("{l}: {}", e.detail),
        None => e.detail.clone(),
    }
}

impl Server {
    /// Deepest workspace / composer root containing `file` that is a Laravel application.
    fn laravel_root(&self, file: &Path) -> Option<PathBuf> {
        self.composers
            .iter()
            .map(|c| &c.root)
            .chain(self.roots.iter())
            .filter(|r| file.starts_with(r))
            .max_by_key(|r| r.components().count())
            .filter(|r| laravel::is_laravel(r))
            .cloned()
    }

    fn laravel_data(&mut self, root: &Path) -> Arc<LaravelData> {
        if let Some((at, d)) = self.laravel.get(root) {
            if at.elapsed() < TTL {
                return d.clone();
            }
        }
        let started = Instant::now();
        let data = {
            let read = |p: &Path| self.text_for(&path_to_uri(p)).map(|t| t.into_owned());
            Arc::new(laravel::build(root, &read))
        };
        log::debug!(
            "laravel: {} routes, {} config keys, {} translations, {} views, {} env keys in {:.1} ms",
            data.routes.len(),
            data.config.len(),
            data.trans.len(),
            data.views.len(),
            data.env.len(),
            started.elapsed().as_secs_f64() * 1000.0
        );
        self.laravel.insert(root.to_path_buf(), (Instant::now(), data.clone()));
        data
    }

    /// Laravel string under the cursor: (literal, kind, current value, project data).
    fn laravel_at(&mut self, uri: &str, offset: usize) -> Option<(StrLit, Kind, String, Arc<LaravelData>)> {
        let (lit, kind, value) = {
            let doc = self.docs.get(uri)?;
            let lit = string_at(&doc.text, offset)?;
            let kind = kind_at(&doc.text, &lit)?;
            (lit.clone(), kind, doc.text[lit.start..lit.end].to_string())
        };
        let root = self.laravel_root(&uri_to_path(uri)?)?;
        let data = self.laravel_data(&root);
        Some((lit, kind, value, data))
    }

    /// Completion items, or `None` when the cursor is not in a Laravel string.
    pub fn laravel_completion(&mut self, uri: &str, offset: usize) -> Option<Vec<CompletionItem>> {
        let (lit, kind, _, data) = self.laravel_at(uri, offset)?;
        let range = self.doc_range(uri, lit.start, lit.end)?;
        let mut items: Vec<CompletionItem> = Vec::new();
        let mut index: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for e in data.entries(kind) {
            if let Some(&i) = index.get(e.key.as_str()) {
                // same translation key in another locale
                let it = &mut items[i];
                if let Some(Documentation::String(s)) = &mut it.documentation {
                    s.push('\n');
                    s.push_str(&describe(e));
                }
                continue;
            }
            index.insert(&e.key, items.len());
            items.push(CompletionItem {
                label: e.key.clone(),
                kind: Some(item_kind(kind)),
                detail: Some(e.detail.clone()),
                documentation: Some(Documentation::String(describe(e))),
                filter_text: Some(e.key.clone()),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit { range, new_text: e.key.clone() })),
                ..Default::default()
            });
        }
        Some(items)
    }

    pub fn laravel_definition(&mut self, uri: &str, offset: usize) -> Option<Vec<Location>> {
        let (_, kind, value, data) = self.laravel_at(uri, offset)?;
        let locs: Vec<Location> =
            data.lookup(kind, &value).into_iter().map(|e| Location { uri: lsp_uri(&e.uri), range: e.range }).collect();
        (!locs.is_empty()).then_some(locs)
    }

    /// Warnings for route / view / config / translation names that do not exist.
    pub fn laravel_diagnostics(&mut self, uri: &str) -> Vec<Diagnostic> {
        if !self.config.laravel_diagnostics
            || !self.config.inspections
            || self.index.is_library(uri)
            || uri.ends_with(".blade.php")
        {
            return Vec::new();
        }
        let Some(root) = uri_to_path(uri).and_then(|p| self.laravel_root(&p)) else { return Vec::new() };
        let data = self.laravel_data(&root);
        let Some(doc) = self.docs.get(uri) else { return Vec::new() };
        unknown_keys(&doc.text, &doc.parse.stmts, &data)
            .into_iter()
            .map(|u| {
                let mut message = format!("{} '{}' not found", label(u.kind), u.key);
                if let Some(s) = &u.suggestion {
                    message.push_str(&format!(" — did you mean '{s}'?"));
                }
                Diagnostic {
                    range: range(&doc.text, &doc.lines, php_syntax::Span::new(u.start, u.end)),
                    severity: Some(DiagnosticSeverity::WARNING),
                    code: Some(NumberOrString::String(format!("laravel-unknown-{}", kind_name(u.kind)))),
                    source: Some("php-lsp".into()),
                    message,
                    data: Some(serde_json::json!(u.suggestion.into_iter().collect::<Vec<_>>())),
                    ..Default::default()
                }
            })
            .collect()
    }

    /// Does editing `uri` change the Laravel data (routes, config, translations, views)?
    pub fn is_laravel_source(&self, uri: &str) -> bool {
        let Some(path) = uri_to_path(uri) else { return false };
        let Some(root) = self.laravel_root(&path) else { return false };
        let Ok(rel) = path.strip_prefix(&root) else { return false };
        let rel = rel.to_string_lossy().replace('\\', "/");
        [
            "routes/",
            "config/",
            "lang/",
            "resources/lang/",
            "resources/views/",
            "app/Providers/",
            "bootstrap/app.php",
            "Modules/",
        ]
        .iter()
        .any(|p| rel.starts_with(p))
    }

    pub fn laravel_hover(&mut self, uri: &str, offset: usize) -> Option<Hover> {
        let (lit, kind, value, data) = self.laravel_at(uri, offset)?;
        let hits = data.lookup(kind, &value);
        let body = if hits.is_empty() {
            format!("**{}** `{value}` — not found", kind_name(kind))
        } else {
            let lines: Vec<String> = hits.iter().map(|e| format!("- {}", describe(e))).collect();
            format!("**{}** `{value}`\n\n{}", kind_name(kind), lines.join("\n"))
        };
        Some(Hover {
            contents: HoverContents::Markup(MarkupContent { kind: MarkupKind::Markdown, value: body }),
            range: self.doc_range(uri, lit.start, lit.end),
        })
    }
}
