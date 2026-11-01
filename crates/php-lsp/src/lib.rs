//! PHP language server. The binary (`main.rs`) runs it over stdio; tests run it over an in-memory
//! connection (`lsp_server::Connection::memory()`).

// `lsp_types::Uri` caches its parsed form internally, which clippy reports as a mutable key in
// `HashMap<Uri, _>`; its hash never changes and lsp-types requires such maps (`WorkspaceEdit`).
#![allow(clippy::mutable_key_type)]

mod convert;
mod features;
mod indexer;
mod laravel;
mod server;
mod texts;

pub use server::run;
