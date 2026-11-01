//! File texts and line indexes loaded on demand while one response is built, so every file is
//! read (and its lines counted) at most once per request.

use crate::convert::*;
use crate::server::Server;
use lsp_types::{Location, Range};
use php_index::SymLoc;
use php_syntax::{LineIndex, Span};
use std::collections::HashMap;

#[derive(Default)]
pub struct Texts(HashMap<String, Option<(String, LineIndex)>>);

impl Texts {
    pub fn get(&mut self, srv: &Server, uri: &str) -> Option<&(String, LineIndex)> {
        self.0
            .entry(uri.to_string())
            .or_insert_with(|| {
                let text = srv.text_for(uri)?.into_owned();
                let lines = LineIndex::new(&text);
                Some((text, lines))
            })
            .as_ref()
    }

    pub fn range(&mut self, srv: &Server, uri: &str, span: Span) -> Option<Range> {
        let (text, lines) = self.get(srv, uri)?;
        Some(range(text, lines, span))
    }

    /// Location of a symbol's name.
    pub fn location(&mut self, srv: &Server, loc: SymLoc) -> Option<Location> {
        let uri = srv.index.uri_of(loc);
        let span = srv.index.sym(loc).name_span;
        Some(Location { uri: lsp_uri(uri), range: self.range(srv, uri, span)? })
    }
}
