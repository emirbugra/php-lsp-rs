//! Diagnostics of open documents: syntax errors, inspections and Laravel warnings.

use super::{Doc, Server};
use crate::convert::*;
use lsp_types::*;
use php_syntax::ParseError;
use serde_json::json;

/// Inspections reported in Blade templates. Left out: undefined functions and constants (directives
/// become calls like `section(…)`, non-ASCII text becomes stray names), argument counts of
/// directives mapped to `view()` (`@each` takes four), unused imports.
const BLADE_CHECKS: &[&str] =
    &["undefined-class", "undefined-method", "undefined-class-constant", "deprecated", "missing-argument"];

impl Server {
    /// Check `uri` completely and publish the result.
    pub(crate) fn publish_diagnostics(&mut self, uri: &str) {
        let checked = self.check(uri);
        let Some(doc) = self.docs.get_mut(uri) else { return };
        doc.checked = checked;
        self.publish_laravel_update(uri);
    }

    /// Recompute only the Laravel warnings of `uri` and publish them with the stored results of
    /// its last full check.
    pub(super) fn publish_laravel_update(&mut self, uri: &str) {
        let laravel = self.laravel_diagnostics(uri);
        let Some(doc) = self.docs.get(uri) else { return };
        let mut diags = doc.checked.clone();
        diags.extend(laravel);
        let params = PublishDiagnosticsParams { uri: lsp_uri(uri), diagnostics: diags, version: Some(doc.version) };
        self.notify("textDocument/publishDiagnostics", serde_json::to_value(params).unwrap());
    }

    /// Syntax errors and inspection results of an open document.
    fn check(&mut self, uri: &str) -> Vec<Diagnostic> {
        let started = std::time::Instant::now();
        let problems = self.problems_for(uri);
        log::debug!(
            "inspections: {} problem(s) in {:.1} ms ({})",
            problems.len(),
            started.elapsed().as_secs_f64() * 1000.0,
            uri
        );
        let Some(doc) = self.docs.get(uri) else { return Vec::new() };
        if doc.source.is_some() {
            // a template: its code is made up (see `php_syntax::blade`), so no syntax errors, and
            // only checks the made-up parts cannot trip
            return problems.into_iter().filter(|p| BLADE_CHECKS.contains(&p.code)).map(|p| problem(doc, p)).collect();
        }
        let mut diags: Vec<Diagnostic> = doc.parse.errors.iter().take(200).map(|e| syntax_error(doc, e)).collect();
        diags.extend(problems.into_iter().map(|p| problem(doc, p)));
        diags
    }

    /// Semantic problems of an open document (empty for vendor / built-in files).
    fn problems_for(&mut self, uri: &str) -> Vec<php_index::Problem> {
        if !self.config.inspections || self.index.is_library(uri) {
            return Vec::new();
        }
        let opts = php_index::InspectOptions { unresolved: !self.indexing };
        for attempt in 0..2 {
            let Some(analysis) = self.analyze_doc(uri) else { return Vec::new() };
            let Some(doc) = self.docs.get(uri) else { return Vec::new() };
            let problems = php_index::inspect(&self.index, uri, &doc.text, &doc.parse, &analysis, &opts);
            if attempt == 1 || !opts.unresolved {
                return problems;
            }
            // classes that exist in vendor/ but were not indexed yet: load them through composer
            let mut missing: Vec<String> = problems
                .iter()
                .filter(|p| p.code == "undefined-class")
                .filter_map(|p| p.args.first().cloned())
                .collect();
            missing.sort();
            missing.dedup();
            let mut loaded = false;
            for fqn in missing {
                loaded |= self.autoload(&fqn);
            }
            if !loaded {
                return problems;
            }
        }
        Vec::new()
    }
}

fn syntax_error(doc: &Doc, e: &ParseError) -> Diagnostic {
    // an empty span (error at end of input) would be invisible
    let mut span = e.span;
    if span.is_empty() {
        span.end = (span.end + 1).min(doc.text.len() as u32);
        if span.start > 0 && span.start == span.end {
            span.start -= 1;
        }
    }
    Diagnostic {
        range: range(&doc.text, &doc.lines, span),
        severity: Some(DiagnosticSeverity::ERROR),
        source: Some("php-lsp".into()),
        message: e.message.clone(),
        ..Default::default()
    }
}

fn problem(doc: &Doc, p: php_index::Problem) -> Diagnostic {
    let mut tags = Vec::new();
    if p.unnecessary {
        tags.push(DiagnosticTag::UNNECESSARY);
    }
    if p.deprecated {
        tags.push(DiagnosticTag::DEPRECATED);
    }
    Diagnostic {
        range: range(&doc.text, &doc.lines, p.span),
        severity: Some(match p.severity {
            php_index::Severity::Error => DiagnosticSeverity::ERROR,
            php_index::Severity::Warning => DiagnosticSeverity::WARNING,
            php_index::Severity::Hint => DiagnosticSeverity::HINT,
        }),
        code: Some(NumberOrString::String(p.code.to_string())),
        source: Some("php-lsp".into()),
        message: p.message,
        tags: if tags.is_empty() { None } else { Some(tags) },
        data: Some(json!(p.args)),
        ..Default::default()
    }
}
