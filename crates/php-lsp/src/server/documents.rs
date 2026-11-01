//! Open documents: open / change / close notifications and files changed on disk.

use super::{Doc, Server};
use crate::convert::*;
use crate::indexer;
use lsp_types::*;
use php_syntax::LineIndex;
use serde_json::json;
use std::borrow::Cow;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Diagnostics are recomputed once typing pauses for this long.
const DEBOUNCE: Duration = Duration::from_millis(250);

impl Server {
    /// Text of an open document, or of the file on disk.
    pub fn text_for(&self, uri: &str) -> Option<Cow<'_, str>> {
        if let Some(d) = self.docs.get(uri) {
            return Some(Cow::Borrowed(d.text.as_str()));
        }
        let path = uri_to_path(uri)?;
        read_text(&path).map(Cow::Owned)
    }

    pub(super) fn did_open(&mut self, p: DidOpenTextDocumentParams) {
        let uri = canon(&p.text_document.uri);
        let text = p.text_document.text;
        let parse = Arc::new(php_syntax::parse(&text));
        let lines = LineIndex::new(&text);
        self.index.update_file(php_index::index_parse(uri.clone(), &text, &parse));
        let version = p.text_document.version;
        self.docs
            .insert(uri.clone(), Doc { text, version, lines, parse, analysis: Mutex::new(None), checked: Vec::new() });
        self.publish_diagnostics(&uri);
    }

    pub(super) fn did_change(&mut self, p: DidChangeTextDocumentParams) {
        let uri = canon(&p.text_document.uri);
        if let Some(doc) = self.docs.get_mut(&uri) {
            for ch in p.content_changes {
                match ch.range {
                    Some(r) => {
                        let s = offset_of(&doc.text, &doc.lines, r.start);
                        let e = offset_of(&doc.text, &doc.lines, r.end);
                        let (s, e) = (s.min(doc.text.len()), e.min(doc.text.len()).max(s));
                        doc.text.replace_range(s..e, &ch.text);
                    }
                    None => doc.text = ch.text,
                }
                doc.lines = LineIndex::new(&doc.text);
            }
            doc.version = p.text_document.version;
        }
        self.reparse(&uri);
    }

    pub(super) fn did_close(&mut self, p: DidCloseTextDocumentParams) {
        let uri = canon(&p.text_document.uri);
        self.docs.remove(&uri);
        // fall back to the on-disk version
        match uri_to_path(&uri).and_then(|p| indexer::index_file(&p)) {
            Some(fd) => self.index.update_file(fd),
            None => self.index.remove_file(&uri),
        }
        self.notify("textDocument/publishDiagnostics", json!({ "uri": uri, "diagnostics": [] }));
    }

    /// Files changed outside the editor (git checkout, composer install …).
    pub(super) fn did_change_watched_files(&mut self, p: DidChangeWatchedFilesParams) {
        let mut laravel_changed = false;
        for ev in p.changes {
            let uri = canon(&ev.uri);
            if self.docs.contains_key(&uri) {
                continue;
            }
            laravel_changed |= self.is_laravel_source(&uri);
            let Some(path) = uri_to_path(&uri) else { continue };
            if !indexer::is_php_path(&path) {
                continue;
            }
            if ev.typ == FileChangeType::DELETED {
                self.index.remove_file(&uri);
            } else if let Some(fd) = indexer::index_file(&path) {
                self.index.update_file(fd);
            }
        }
        if laravel_changed {
            self.laravel.clear();
            self.republish_except(&[]);
        }
    }

    /// Parse and index an edited document now; its diagnostics follow after the debounce.
    fn reparse(&mut self, uri: &str) {
        let Some(doc) = self.docs.get_mut(uri) else { return };
        let parse = Arc::new(php_syntax::parse(&doc.text));
        doc.lines = LineIndex::new(&doc.text);
        let data = php_index::index_parse(uri.to_string(), &doc.text, &parse);
        doc.parse = parse;
        self.index.update_file(data);
        self.flush_at = Some(Instant::now() + DEBOUNCE);
        if !self.pending.iter().any(|u| u == uri) {
            self.pending.push(uri.to_string());
        }
    }

    /// Publish the diagnostics of the documents edited since the last flush.
    pub(super) fn flush_pending(&mut self) {
        self.flush_at = None;
        let pending = std::mem::take(&mut self.pending);
        let laravel_changed = pending.iter().any(|u| self.is_laravel_source(u));
        if laravel_changed {
            self.laravel.clear();
        }
        for uri in &pending {
            self.publish_diagnostics(uri);
        }
        if laravel_changed {
            self.republish_except(&pending);
        }
    }

    /// Recompute diagnostics of the open documents not in `skip` (Laravel names they use may have
    /// appeared or disappeared).
    fn republish_except(&mut self, skip: &[String]) {
        let open: Vec<String> = self.docs.keys().filter(|u| !skip.contains(u)).cloned().collect();
        for uri in open {
            self.publish_laravel_update(&uri);
        }
    }
}
