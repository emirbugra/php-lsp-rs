//! The server: state, the message loop and request dispatch.
//!
//! - `capabilities.rs`: the `initialize` answer
//! - `documents.rs`: open / change / close and files changed on disk
//! - `diagnostics.rs`: syntax errors, inspections and Laravel warnings of open documents

mod capabilities;
mod diagnostics;
mod documents;

use crate::convert::*;
use crate::indexer::{self, IndexMsg};
use crossbeam_channel::{bounded, select, Receiver};
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, RequestId, Response, ResponseError};
use lsp_types::*;
use php_composer::Composer;
use php_index::{Analysis, Index, Origins};
use php_syntax::{LineIndex, Parse};
use rustc_hash::FxHashMap;
use serde_json::{json, Value};
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub struct Doc {
    /// The text features work on: the document itself, or for a Blade template the PHP code made
    /// of it (same length, lines and columns; see `php_syntax::blade`).
    pub text: String,
    /// The template text of a Blade document (edits apply to it; `text` is rebuilt from it).
    pub source: Option<String>,
    pub version: i32,
    pub lines: LineIndex,
    pub parse: Arc<Parse>,
    /// Last analysis, valid while the document version and the index generation are unchanged.
    analysis: Mutex<Option<(AnalysisKey, Arc<Analysis>)>>,
    /// Syntax errors and inspection results of the last full check (everything but the Laravel
    /// warnings, which can change when another file changes).
    checked: Vec<Diagnostic>,
}

/// (index generation, document version)
type AnalysisKey = (u64, i32);

/// Variables a view starts with, by name.
pub type ViewVars = Arc<Vec<(String, php_index::types::Types)>>;

pub struct Config {
    pub index_vendor: bool,
    pub inspections: bool,
    /// `auto` | `builtin` | `pint` | `php-cs-fixer` | `off`
    pub formatter: String,
    /// Explicit formatter executable (overrides detection).
    pub formatter_path: String,
    /// Warn about unknown Laravel route / view / config / translation names.
    pub laravel_diagnostics: bool,
    /// PHP version for suggested types (`8.2`); empty = from composer.json / the installed `php`.
    pub php_version: String,
    /// Settings of the built-in formatter (`formatPreset`, `formatOptions`, `useProjectCodeStyle`).
    pub format: crate::features::FormatSettings,
    /// Inlay hints with parameter names at call sites.
    pub inlay_parameter_names: bool,
    /// Inlay hints with the inferred type of variables where they are first assigned.
    pub inlay_variable_types: bool,
    /// Reference / implementation counts above classes and methods.
    pub code_lens: bool,
}

pub struct Server {
    pub conn: Connection,
    pub index: Index,
    pub docs: FxHashMap<String, Doc>,
    pub roots: Vec<PathBuf>,
    pub composers: Vec<Composer>,
    pub config: Config,
    pub pool: rayon::ThreadPool,
    /// Documents whose diagnostics are due once typing pauses.
    pending: Vec<String>,
    /// When `pending` is flushed: 250 ms after the last edit. Other messages (index progress)
    /// must not push it back.
    flush_at: Option<std::time::Instant>,
    next_id: i32,
    indexing: bool,
    last_pct: i64,
    /// `workspace/*/refresh` requests the client supports, sent when indexing finishes so lenses and
    /// hints computed against a half-built index are redone.
    refresh: Vec<&'static str>,
    /// Variables of Blade views (by template URI), with the time they were collected.
    pub view_vars: Mutex<FxHashMap<String, (std::time::Instant, ViewVars)>>,
    /// Laravel route / config / translation / view / env data per project root, with build time.
    pub laravel: FxHashMap<PathBuf, (std::time::Instant, std::sync::Arc<crate::laravel::LaravelData>)>,
}

type Res<T> = Result<T, Box<dyn std::error::Error + Sync + Send>>;

fn error(code: ErrorCode, message: String) -> ResponseError {
    ResponseError { code: code as i32, message, data: None }
}

/// Request parameters, or an `InvalidParams` error the client can show.
fn params<P: serde::de::DeserializeOwned>(v: Value) -> Result<P, ResponseError> {
    serde_json::from_value(v).map_err(|e| error(ErrorCode::InvalidParams, e.to_string()))
}

pub fn run(conn: Connection) -> Res<()> {
    let (id, params) = conn.initialize_start()?;
    let init: InitializeParams = serde_json::from_value(params)?;
    let roots = workspace_roots(&init);
    let config = Config::from_options(init.initialization_options.as_ref().unwrap_or(&Value::Null));
    let refresh = refresh_support(&init);
    conn.initialize_finish(id, capabilities::initialize_result())?;

    let composers: Vec<Composer> = roots.iter().flat_map(|r| php_composer::discover(r)).collect();
    log::info!("workspace roots: {:?}, composer projects: {}", roots, composers.len());
    let mut index = index_with_stubs(&roots, &composers);
    let aliases: Vec<(String, String)> = composers
        .iter()
        .map(|c| &c.root)
        .chain(roots.iter())
        .filter(|r| crate::laravel::is_laravel(r))
        .flat_map(|r| crate::laravel::aliases(r))
        .collect();
    if !aliases.is_empty() {
        index.set_class_aliases(aliases);
    }

    let (tx, rx) = bounded::<IndexMsg>(indexer::QUEUE);
    indexer::spawn(roots.clone(), index.origins().clone(), config.index_vendor, tx);

    let mut server = Server {
        conn,
        index,
        docs: FxHashMap::default(),
        roots,
        composers,
        config,
        pool: rayon::ThreadPoolBuilder::new()
            .num_threads(std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2))
            .stack_size(64 << 20)
            .build()?,
        pending: Vec::new(),
        flush_at: None,
        next_id: 1000,
        indexing: true,
        last_pct: -1,
        refresh,
        view_vars: Mutex::new(FxHashMap::default()),
        laravel: FxHashMap::default(),
    };
    server.create_progress();
    server.main_loop(rx)
}

fn refresh_support(init: &InitializeParams) -> Vec<&'static str> {
    let ws = init.capabilities.workspace.as_ref();
    let mut out = Vec::new();
    if ws.and_then(|w| w.code_lens.as_ref()).and_then(|c| c.refresh_support).unwrap_or(false) {
        out.push("workspace/codeLens/refresh");
    }
    if ws.and_then(|w| w.inlay_hint.as_ref()).and_then(|c| c.refresh_support).unwrap_or(false) {
        out.push("workspace/inlayHint/refresh");
    }
    out
}

/// Workspace folders, or the root URI of clients that send only that.
fn workspace_roots(init: &InitializeParams) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(folders) = &init.workspace_folders {
        for f in folders {
            if let Some(p) = uri_to_path(f.uri.as_str()) {
                roots.push(p);
            }
        }
    }
    #[allow(deprecated)]
    if roots.is_empty() {
        if let Some(u) = &init.root_uri {
            if let Some(p) = uri_to_path(u.as_str()) {
                roots.push(p);
            }
        }
    }
    roots
}

impl Config {
    /// From the client's `initializationOptions` (the VS Code settings).
    fn from_options(opts: &Value) -> Config {
        Config {
            index_vendor: opts.get("indexVendor").and_then(|v| v.as_bool()).unwrap_or(true),
            inspections: opts.get("inspections").and_then(|v| v.as_bool()).unwrap_or(true),
            formatter: opts.get("formatter").and_then(|v| v.as_str()).unwrap_or("auto").to_string(),
            formatter_path: opts.get("formatterPath").and_then(|v| v.as_str()).unwrap_or("").trim().to_string(),
            laravel_diagnostics: opts.get("laravelDiagnostics").and_then(|v| v.as_bool()).unwrap_or(true),
            php_version: opts.get("phpVersion").and_then(|v| v.as_str()).unwrap_or("").trim().to_string(),
            format: crate::features::FormatSettings::from_options(opts),
            inlay_parameter_names: true,
            inlay_variable_types: true,
            code_lens: true,
        }
        .with_live(opts)
    }

    fn with_live(mut self, opts: &Value) -> Config {
        self.update_live(opts);
        self
    }

    /// Take the settings that apply without a restart from `opts` (initialization options or a
    /// `workspace/didChangeConfiguration` payload); absent keys keep their value.
    pub fn update_live(&mut self, opts: &Value) {
        let flag = |key: &str, old: bool| opts.get(key).and_then(|v| v.as_bool()).unwrap_or(old);
        self.inlay_parameter_names = flag("inlayParameterNames", self.inlay_parameter_names);
        self.inlay_variable_types = flag("inlayVariableTypes", self.inlay_variable_types);
        self.code_lens = flag("codeLens", self.code_lens);
    }
}

/// An index that knows where files come from and already holds the built-in stubs; the project
/// files are added by the background indexer.
fn index_with_stubs(roots: &[PathBuf], composers: &[Composer]) -> Index {
    let stub_dir = php_stubs::ensure_extracted().map_err(|e| log::warn!("could not unpack built-in stubs: {e}")).ok();
    let mut index = Index::new();
    index.set_origins(Origins::new(
        roots.iter().map(|r| path_to_uri(r)).collect(),
        composers.iter().map(|c| path_to_uri(&c.vendor_dir)).collect(),
        stub_dir.as_deref().map(path_to_uri),
    ));
    if let Some(dir) = stub_dir {
        let (stub_files, _) = indexer::collect_files(&[dir], index.origins());
        for f in stub_files {
            if let Some(fd) = indexer::index_file(&f) {
                index.update_file(fd);
            }
        }
        log::info!("indexed built-in stubs: {} symbols", index.symbol_count());
    }
    index
}

impl Server {
    fn main_loop(&mut self, rx: Receiver<IndexMsg>) -> Res<()> {
        let receiver = self.conn.receiver.clone();
        let mut index_rx = Some(rx);
        loop {
            let never = crossbeam_channel::never();
            let irx = index_rx.as_ref().unwrap_or(&never);
            let debounce: Receiver<std::time::Instant> = match self.flush_at {
                Some(at) => crossbeam_channel::at(at),
                None => crossbeam_channel::never(),
            };
            select! {
                recv(debounce) -> _ => self.flush_pending(),
                recv(receiver) -> msg => {
                    let Ok(msg) = msg else { return Ok(()) };
                    match msg {
                        Message::Request(req) => {
                            if self.conn.handle_shutdown(&req)? {
                                return Ok(());
                            }
                            self.on_request(req);
                        }
                        Message::Notification(n) => {
                            if n.method == "exit" {
                                return Ok(());
                            }
                            let method = n.method.clone();
                            let r = std::panic::catch_unwind(AssertUnwindSafe(|| self.on_notification(n)));
                            if r.is_err() {
                                log::error!("panic while handling notification {method}");
                            }
                        }
                        Message::Response(_) => {}
                    }
                }
                recv(irx) -> msg => {
                    match msg {
                        Ok(m) => self.on_index_msg(m),
                        Err(_) => index_rx = None,
                    }
                }
            }
        }
    }

    // ---- plumbing ------------------------------------------------------

    pub fn show_message(&self, typ: MessageType, msg: &str) {
        self.notify("window/showMessage", json!({ "type": typ, "message": msg }));
    }

    pub fn send(&self, msg: Message) {
        let _ = self.conn.sender.send(msg);
    }

    pub fn notify(&self, method: &str, params: Value) {
        self.send(Message::Notification(Notification { method: method.to_string(), params }));
    }

    fn request(&mut self, method: &str, params: Value) {
        self.next_id += 1;
        let id = RequestId::from(self.next_id);
        self.send(Message::Request(Request { id, method: method.to_string(), params }));
    }

    fn create_progress(&mut self) {
        self.request("window/workDoneProgress/create", json!({ "token": "php-lsp/index" }));
        self.notify(
            "$/progress",
            json!({ "token": "php-lsp/index", "value": { "kind": "begin", "title": "PHP indexing", "percentage": 0 } }),
        );
    }

    fn on_index_msg(&mut self, msg: IndexMsg) {
        match msg {
            IndexMsg::Batch { files, done, total } => {
                for f in files {
                    // an open buffer is always more current than disk
                    if !self.docs.contains_key(&*f.uri) {
                        self.index.update_file(f.compact());
                    }
                }
                self.report_progress(done, total);
            }
            IndexMsg::Started { total } => self.report_progress(0, total),
            IndexMsg::Done { files } => {
                self.indexing = false;
                unsafe { libmimalloc_sys::mi_collect(true) };
                log::info!("indexed {} files, {} symbols", self.index.file_count(), self.index.symbol_count());
                self.notify(
                    "$/progress",
                    json!({ "token": "php-lsp/index", "value": { "kind": "end", "message": format!("{files} files indexed") } }),
                );
                // names can be resolved reliably now
                let open: Vec<String> = self.docs.keys().cloned().collect();
                for uri in open {
                    self.publish_diagnostics(&uri);
                }
                for method in self.refresh.clone() {
                    self.request(method, Value::Null);
                }
            }
        }
    }

    fn report_progress(&mut self, done: usize, total: usize) {
        let pct = (done * 100).checked_div(total).unwrap_or(100) as i64;
        if pct != self.last_pct && self.indexing {
            self.last_pct = pct;
            self.notify(
                "$/progress",
                json!({ "token": "php-lsp/index", "value": {
                    "kind": "report", "percentage": pct, "message": format!("{done}/{total} files")
                } }),
            );
        }
    }

    fn on_notification(&mut self, n: Notification) {
        match n.method.as_str() {
            "textDocument/didOpen" => {
                if let Ok(p) = serde_json::from_value(n.params) {
                    self.did_open(p);
                }
            }
            "textDocument/didChange" => {
                if let Ok(p) = serde_json::from_value(n.params) {
                    self.did_change(p);
                }
            }
            "textDocument/didClose" => {
                if let Ok(p) = serde_json::from_value(n.params) {
                    self.did_close(p);
                }
            }
            // the client sends the format settings when they change (no restart needed)
            "workspace/didChangeConfiguration" => {
                if let Some(settings) = n.params.get("settings") {
                    self.config.format.update(settings);
                    self.config.update_live(settings);
                }
            }
            "workspace/didChangeWatchedFiles" => {
                if let Ok(p) = serde_json::from_value(n.params) {
                    self.did_change_watched_files(p);
                }
            }
            _ => {}
        }
    }

    fn on_request(&mut self, req: Request) {
        let id = req.id.clone();
        let method = req.method.clone();
        let started = std::time::Instant::now();
        let params_dbg = if log::log_enabled!(log::Level::Debug) { req.params.to_string() } else { String::new() };
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| self.dispatch(req))).unwrap_or_else(|_| {
            Err(error(ErrorCode::InternalError, format!("internal error while handling {method}")))
        });
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        let summary = match &result {
            Ok(None | Some(Value::Null)) => "null".to_string(),
            Ok(Some(Value::Array(a))) => format!("{} results", a.len()),
            Ok(Some(Value::Object(o))) => match o.get("items").and_then(|i| i.as_array()) {
                Some(i) => format!("{} items", i.len()),
                None => "ok".to_string(),
            },
            Ok(Some(_)) => "ok".to_string(),
            Err(e) => format!("error {}: {}", e.code, e.message),
        };
        match &result {
            Err(e) if e.code == ErrorCode::InternalError as i32 => {
                log::error!("{method} panicked after {ms:.1} ms | {params_dbg}")
            }
            _ if ms > 200.0 => log::warn!("{method} took {ms:.0} ms ({summary})"),
            _ => log::debug!(
                "{method} -> {summary} in {ms:.1} ms | {}",
                params_dbg.chars().take(300).collect::<String>()
            ),
        }
        let resp = match result {
            Ok(v) => Response::new_ok(id, v.unwrap_or(Value::Null)),
            Err(e) => Response { id, response_result: Err(e) },
        };
        self.send(Message::Response(resp));
    }

    fn dispatch(&mut self, req: Request) -> Result<Option<Value>, ResponseError> {
        match req.method.as_str() {
            "textDocument/definition" => Ok(self.definition(params(req.params)?)),
            "textDocument/hover" => Ok(self.hover(params(req.params)?)),
            "textDocument/completion" => Ok(self.completion(params(req.params)?)),
            "textDocument/documentSymbol" => Ok(self.document_symbols(params(req.params)?)),
            "workspace/symbol" => Ok(self.workspace_symbols(params(req.params)?)),
            "phpLsp/importCodeStyle" => {
                self.import_code_style(params(req.params)?).map(Some).map_err(|e| error(ErrorCode::InvalidParams, e))
            }
            "phpLsp/exportCodeStyle" => {
                self.export_code_style(params(req.params)?).map(Some).map_err(|e| error(ErrorCode::InvalidParams, e))
            }
            "phpLsp/moveClass" => {
                self.move_class(params(req.params)?).map(Some).map_err(|e| error(ErrorCode::InvalidParams, e))
            }
            "phpLsp/status" => Ok(Some(json!({
                "version": env!("CARGO_PKG_VERSION"),
                "indexing": self.indexing,
                "percent": self.last_pct.max(0),
                "files": self.index.file_count(),
                "symbols": self.index.symbol_count(),
                "openDocuments": self.docs.len(),
                "roots": self.roots.iter().map(|r| r.to_string_lossy().into_owned()).collect::<Vec<_>>(),
                "composerProjects": self.composers.iter().map(|c| c.root.to_string_lossy().into_owned()).collect::<Vec<_>>(),
                "indexVendor": self.config.index_vendor,
                "inspections": self.config.inspections,
            }))),
            "textDocument/codeAction" => Ok(self.code_actions(params(req.params)?)),
            "textDocument/references" => Ok(self.references(params(req.params)?)),
            "textDocument/documentHighlight" => Ok(self.document_highlight(params(req.params)?)),
            "textDocument/prepareRename" => Ok(self.prepare_rename(params(req.params)?)),
            "textDocument/rename" => Ok(self.rename(params(req.params)?)),
            "textDocument/formatting" => Ok(self.formatting(params(req.params)?)),
            "textDocument/rangeFormatting" => Ok(self.range_formatting(params(req.params)?)),
            "textDocument/onTypeFormatting" => Ok(self.on_type_formatting(params(req.params)?)),
            "textDocument/inlayHint" => Ok(self.inlay_hints(params(req.params)?)),
            "textDocument/codeLens" => Ok(self.code_lens(params(req.params)?)),
            "codeLens/resolve" => Ok(self.code_lens_resolve(params(req.params)?)),
            "textDocument/typeDefinition" => Ok(self.type_definition(params(req.params)?)),
            "textDocument/prepareCallHierarchy" => Ok(self.prepare_call_hierarchy(params(req.params)?)),
            "callHierarchy/incomingCalls" => Ok(self.incoming_calls(params(req.params)?)),
            "callHierarchy/outgoingCalls" => Ok(self.outgoing_calls(params(req.params)?)),
            "textDocument/implementation" => Ok(self.implementation(params(req.params)?)),
            "textDocument/prepareTypeHierarchy" => Ok(self.prepare_type_hierarchy(params(req.params)?)),
            "typeHierarchy/supertypes" => Ok(self.type_supertypes(params(req.params)?)),
            "typeHierarchy/subtypes" => Ok(self.type_subtypes(params(req.params)?)),
            "textDocument/signatureHelp" => Ok(self.signature_help(params(req.params)?)),
            other => Err(error(ErrorCode::MethodNotFound, format!("unknown request '{other}'"))),
        }
    }

    // ---- shared helpers for features ----------------------------------

    /// Analysis of an open document; reused until the document or the index changes.
    pub fn analyze_doc(&self, uri: &str) -> Option<Arc<Analysis>> {
        let doc = self.docs.get(uri)?;
        let key = (self.index.generation(), doc.version);
        let mut cached = doc.analysis.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((k, a)) = &*cached {
            if *k == key {
                return Some(a.clone());
            }
        }
        let vars = self.view_variables(uri);
        let analysis = Arc::new(php_index::analyze_with(&self.index, &doc.text, &doc.parse.stmts, &vars, &[]));
        *cached = Some((key, analysis.clone()));
        Some(analysis)
    }

    /// Try to load a class through composer autoloading when it is not indexed yet.
    pub fn autoload(&mut self, fqn: &str) -> bool {
        if self.index.find_class(fqn).is_some() {
            return true;
        }
        for c in &self.composers {
            if let Some(path) = c.class_path(fqn) {
                if let Some(fd) = indexer::index_file(&path) {
                    self.index.update_file(fd);
                    return self.index.find_class(fqn).is_some();
                }
            }
        }
        false
    }
}
