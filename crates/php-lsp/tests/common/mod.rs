//! A small LSP client for the end-to-end tests: starts the server on an in-memory connection,
//! opens files of a fixture project and sends requests.

#![allow(dead_code)]

use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Servers are started one at a time: the first one unpacks the built-in stubs into the cache dir.
static START: Mutex<()> = Mutex::new(());

const TIMEOUT: Duration = Duration::from_secs(120);

pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

pub fn uri(path: &Path) -> String {
    format!("file://{}", path.display())
}

/// LSP position of `needle` (its `nth` occurrence, 0-based), moved right by `delta` characters.
pub fn pos(text: &str, needle: &str, nth: usize, delta: usize) -> Value {
    let mut at = text.find(needle).unwrap_or_else(|| panic!("{needle:?} not in text"));
    for _ in 0..nth {
        at = at + 1 + text[at + 1..].find(needle).unwrap_or_else(|| panic!("{needle:?} #{nth} not in text"));
    }
    at += delta;
    let line = text[..at].matches('\n').count();
    let col = at - text[..at].rfind('\n').map(|i| i + 1).unwrap_or(0);
    json!({ "line": line, "character": col })
}

pub struct Client {
    pub root: PathBuf,
    conn: Connection,
    server: Option<JoinHandle<()>>,
    next_id: i32,
    /// Every notification the server sent so far.
    pub notifications: Vec<Notification>,
}

impl Client {
    /// Start a server for `root` and wait until the workspace is indexed.
    pub fn start(root: &Path) -> Client {
        let _guard = START.lock().unwrap_or_else(|e| e.into_inner());
        let (client, server) = Connection::memory();
        let handle = std::thread::Builder::new()
            .stack_size(128 << 20)
            .spawn(move || php_lsp::run(server).expect("server failed"))
            .unwrap();
        let mut c = Client {
            root: root.to_path_buf(),
            conn: client,
            server: Some(handle),
            next_id: 0,
            notifications: Vec::new(),
        };
        let root_uri = uri(root);
        c.request(
            "initialize",
            json!({
                "processId": null,
                "rootUri": root_uri,
                "workspaceFolders": [{ "uri": root_uri, "name": "test" }],
                "capabilities": {},
                "initializationOptions": { "indexVendor": true },
            }),
        );
        c.notify("initialized", json!({}));
        c.wait_for(|n| {
            n.method == "$/progress" && n.params["token"] == "php-lsp/index" && n.params["value"]["kind"] == "end"
        });
        c
    }

    pub fn notify(&self, method: &str, params: Value) {
        self.conn.sender.send(Message::Notification(Notification::new(method.into(), params))).unwrap();
    }

    /// Handle one incoming message: store notifications, answer server-to-client requests.
    fn receive(&mut self) -> Option<Response> {
        let msg = self.conn.receiver.recv_timeout(TIMEOUT).expect("server did not answer in time");
        match msg {
            Message::Notification(n) => self.notifications.push(n),
            Message::Request(r) => {
                self.conn.sender.send(Message::Response(Response::new_ok(r.id, Value::Null))).unwrap();
            }
            Message::Response(r) => return Some(r),
        }
        None
    }

    /// Send a request and return its full response (result or error).
    pub fn request_raw(&mut self, method: &str, params: Value) -> Response {
        self.next_id += 1;
        let id = RequestId::from(self.next_id);
        self.conn.sender.send(Message::Request(Request::new(id.clone(), method.into(), params))).unwrap();
        loop {
            if let Some(r) = self.receive() {
                if r.id == id {
                    return r;
                }
            }
        }
    }

    pub fn request(&mut self, method: &str, params: Value) -> Value {
        match self.request_raw(method, params).response_result {
            Ok(v) => v,
            Err(e) => panic!("{method} failed: {} ({})", e.message, e.code),
        }
    }

    fn wait_for(&mut self, done: impl Fn(&Notification) -> bool) {
        let end = Instant::now() + TIMEOUT;
        while !self.notifications.iter().any(&done) {
            assert!(Instant::now() < end, "timed out waiting for a notification");
            self.receive();
        }
    }

    /// Open `rel` (relative to the root) with its on-disk text, or with `text` when given.
    pub fn open(&mut self, rel: &str, text: Option<&str>) -> (String, String) {
        let path = self.root.join(rel);
        let text = match text {
            Some(t) => t.to_string(),
            None => std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("cannot read {}", path.display())),
        };
        let u = uri(&path);
        self.notify(
            "textDocument/didOpen",
            json!({ "textDocument": { "uri": u, "languageId": "php", "version": 1, "text": text } }),
        );
        (u, text)
    }

    /// Replace the whole text of an open document.
    pub fn change(&mut self, uri: &str, version: i32, text: &str) {
        self.notify(
            "textDocument/didChange",
            json!({ "textDocument": { "uri": uri, "version": version }, "contentChanges": [{ "text": text }] }),
        );
    }

    /// The latest diagnostics published for `uri`, after every message sent so far was handled.
    pub fn diagnostics(&mut self, uri: &str) -> Vec<Value> {
        // the server handles messages in order, so a round trip flushes everything before it
        self.request("workspace/symbol", json!({ "query": "zzzz_sync" }));
        self.notifications
            .iter()
            .rev()
            .find(|n| n.method == "textDocument/publishDiagnostics" && n.params["uri"] == uri)
            .map(|n| n.params["diagnostics"].as_array().cloned().unwrap_or_default())
            .unwrap_or_default()
    }

    /// Like `diagnostics`, but waits until the debounced re-check after an edit has run.
    pub fn diagnostics_after_edit(&mut self, uri: &str) -> Vec<Value> {
        let before = self.notifications.len();
        let end = Instant::now() + TIMEOUT;
        while !self.notifications[before..]
            .iter()
            .any(|n| n.method == "textDocument/publishDiagnostics" && n.params["uri"] == uri)
        {
            assert!(Instant::now() < end, "no diagnostics for {uri}");
            if let Ok(m) = self.conn.receiver.recv_timeout(Duration::from_millis(50)) {
                match m {
                    Message::Notification(n) => self.notifications.push(n),
                    Message::Request(r) => {
                        self.conn.sender.send(Message::Response(Response::new_ok(r.id, Value::Null))).unwrap()
                    }
                    Message::Response(_) => {}
                }
            }
        }
        self.diagnostics(uri)
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        self.request("shutdown", Value::Null);
        self.notify("exit", Value::Null);
        if let Some(h) = self.server.take() {
            let _ = h.join();
        }
    }
}

/// `(file name, 1-based line)` of every location in a definition / implementation result.
pub fn locations(v: &Value) -> Vec<(String, u64)> {
    let list = match v {
        Value::Array(a) => a.clone(),
        Value::Object(_) => vec![v.clone()],
        _ => Vec::new(),
    };
    list.iter()
        .map(|l| {
            let u = l["uri"].as_str().or(l["targetUri"].as_str()).unwrap_or("");
            let r = if l["range"].is_null() { &l["targetSelectionRange"] } else { &l["range"] };
            (u.rsplit('/').next().unwrap_or("").to_string(), r["start"]["line"].as_u64().unwrap_or(0) + 1)
        })
        .collect()
}
