//! A running language server.
//!
//! [`Client::start`] blocks until the `initialize` handshake is done, so call it from a background
//! thread. After that nothing blocks: requests take a callback that runs on the reader thread, and
//! server pushes (diagnostics, progress, exit) arrive through the `on_event` callback given at start.

use std::collections::{HashMap, HashSet};
use std::io::{self, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use crate::frame::{read_message, write_message};
use crate::types::{self, CompletionItem, Diagnostic, Location, Position};

/// Something the server told us without being asked.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The full set of diagnostics for a file (empty clears them).
    Diagnostics { path: PathBuf, diagnostics: Vec<Diagnostic> },
    /// What the server is busy with ("Indexing 43%"), or `None` when it is idle.
    Progress(Option<String>),
    /// The server process ended.
    Exited,
}

type Callback = Box<dyn FnOnce(Result<Value, String>) + Send>;

struct Shared {
    stdin: Mutex<ChildStdin>,
    pending: Mutex<HashMap<i64, Callback>>,
    next_id: AtomicI64,
    alive: AtomicBool,
    versions: Mutex<HashMap<PathBuf, i32>>,
}

impl Shared {
    fn send(&self, msg: &Value) -> io::Result<()> {
        let mut w = self.stdin.lock().unwrap();
        write_message(&mut *w, msg).inspect_err(|_| self.alive.store(false, Ordering::SeqCst))
    }
}

pub struct Client {
    shared: Arc<Shared>,
    child: Mutex<Child>,
    capabilities: Value,
}

impl Client {
    /// Start `program` with `args` in `root` and finish the handshake (or fail after `timeout`).
    pub fn start(
        program: &Path,
        args: &[&str],
        root: &Path,
        path_env: &std::ffi::OsStr,
        timeout: Duration,
        on_event: impl Fn(Event) + Send + Sync + 'static,
    ) -> Result<Client, String> {
        let mut child = Command::new(program)
            .args(args)
            .current_dir(root)
            .env("PATH", path_env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("could not start {}: {e}", program.display()))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let shared = Arc::new(Shared {
            stdin: Mutex::new(stdin),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicI64::new(1),
            alive: AtomicBool::new(true),
            versions: Mutex::new(HashMap::new()),
        });

        let reader_shared = shared.clone();
        std::thread::Builder::new()
            .name("lsp-reader".into())
            .spawn(move || reader(reader_shared, BufReader::new(stdout), on_event))
            .map_err(|e| e.to_string())?;

        let mut client = Client { shared, child: Mutex::new(child), capabilities: Value::Null };
        let root_uri = types::uri_from_path(root);
        let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let init = json!({
            "processId": std::process::id(),
            "clientInfo": {"name": "mide"},
            "rootUri": root_uri,
            "workspaceFolders": [{"uri": root_uri, "name": name}],
            "capabilities": {
                "general": {"positionEncodings": ["utf-16"]},
                "workspace": {"configuration": true, "workspaceFolders": true},
                "window": {"workDoneProgress": true},
                "textDocument": {
                    "synchronization": {"didSave": true},
                    "publishDiagnostics": {"relatedInformation": false},
                    "completion": {"completionItem": {"snippetSupport": false, "documentationFormat": ["plaintext"]}},
                    "hover": {"contentFormat": ["plaintext", "markdown"]},
                    "definition": {"linkSupport": true}
                }
            }
        });
        let result = client.request_blocking("initialize", init, timeout)?;
        let capabilities = result.get("capabilities").cloned().unwrap_or(Value::Null);
        client.notify("initialized", json!({}));
        client.capabilities = capabilities;
        Ok(client)
    }

    pub fn is_alive(&self) -> bool {
        self.shared.alive.load(Ordering::SeqCst)
    }

    /// What the server said it can do (the `capabilities` of the initialize result).
    pub fn capabilities(&self) -> &Value {
        &self.capabilities
    }

    pub fn supports(&self, capability: &str) -> bool {
        match self.capabilities.get(capability) {
            None | Some(Value::Null) | Some(Value::Bool(false)) => false,
            Some(_) => true,
        }
    }

    pub fn notify(&self, method: &str, params: Value) {
        let _ = self.shared.send(&json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    /// Send a request; `cb` runs on the reader thread with the result or an error message.
    pub fn request(&self, method: &str, params: Value, cb: impl FnOnce(Result<Value, String>) + Send + 'static) {
        let id = self.shared.next_id.fetch_add(1, Ordering::SeqCst);
        if !self.is_alive() {
            return cb(Err("the language server is not running".into()));
        }
        self.shared.pending.lock().unwrap().insert(id, Box::new(cb));
        if let Err(e) = self.shared.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})) {
            if let Some(cb) = self.shared.pending.lock().unwrap().remove(&id) {
                cb(Err(e.to_string()));
            }
        }
    }

    pub fn request_blocking(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        let (tx, rx) = mpsc::channel();
        self.request(method, params, move |r| {
            let _ = tx.send(r);
        });
        rx.recv_timeout(timeout).map_err(|_| format!("{method} timed out"))?
    }

    // ---- documents (always sent whole: valid for every server, whatever sync kind it asked for) ----

    pub fn did_open(&self, path: &Path, language_id: &str, text: &str) {
        self.shared.versions.lock().unwrap().insert(path.to_path_buf(), 1);
        self.notify(
            "textDocument/didOpen",
            json!({"textDocument": {"uri": types::uri_from_path(path), "languageId": language_id, "version": 1, "text": text}}),
        );
    }

    pub fn did_change(&self, path: &Path, text: &str) {
        let version = {
            let mut v = self.shared.versions.lock().unwrap();
            let n = v.entry(path.to_path_buf()).or_insert(1);
            *n += 1;
            *n
        };
        self.notify(
            "textDocument/didChange",
            json!({"textDocument": {"uri": types::uri_from_path(path), "version": version}, "contentChanges": [{"text": text}]}),
        );
    }

    pub fn did_save(&self, path: &Path) {
        self.notify("textDocument/didSave", json!({"textDocument": {"uri": types::uri_from_path(path)}}));
    }

    pub fn did_close(&self, path: &Path) {
        self.shared.versions.lock().unwrap().remove(path);
        self.notify("textDocument/didClose", json!({"textDocument": {"uri": types::uri_from_path(path)}}));
    }

    pub fn is_open(&self, path: &Path) -> bool {
        self.shared.versions.lock().unwrap().contains_key(path)
    }

    // ---- language features ----------------------------------------------------------------------

    fn at(path: &Path, pos: Position) -> Value {
        json!({"textDocument": {"uri": types::uri_from_path(path)}, "position": {"line": pos.line, "character": pos.character}})
    }

    pub fn definition(&self, path: &Path, pos: Position, cb: impl FnOnce(Result<Vec<Location>, String>) + Send + 'static) {
        self.request("textDocument/definition", Self::at(path, pos), move |r| cb(r.map(|v| types::parse_locations(&v))));
    }

    pub fn completion(&self, path: &Path, pos: Position, cb: impl FnOnce(Result<Vec<CompletionItem>, String>) + Send + 'static) {
        self.request("textDocument/completion", Self::at(path, pos), move |r| cb(r.map(|v| types::parse_completion(&v))));
    }

    pub fn hover(&self, path: &Path, pos: Position, cb: impl FnOnce(Result<Option<String>, String>) + Send + 'static) {
        self.request("textDocument/hover", Self::at(path, pos), move |r| cb(r.map(|v| types::parse_hover(&v))));
    }

    /// Ask the server to stop, wait briefly, then make sure the process is gone.
    pub fn shutdown(self) {
        let _ = self.request_blocking("shutdown", Value::Null, Duration::from_secs(2));
        self.notify("exit", Value::Null);
        for _ in 0..20 {
            if matches!(self.child.lock().unwrap().try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        // Drop kills it.
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.shared.alive.store(false, Ordering::SeqCst);
        if let Ok(mut c) = self.child.lock() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// The reader thread: routes responses to their callbacks, server requests to canned answers,
/// notifications to `on_event`.
fn reader(shared: Arc<Shared>, mut r: BufReader<std::process::ChildStdout>, on_event: impl Fn(Event)) {
    let mut progress: HashSet<String> = HashSet::new();
    while let Ok(Some(msg)) = read_message(&mut r) {
        let id = msg.get("id");
        let method = msg.get("method").and_then(Value::as_str);
        match (id, method) {
            // A request from the server: answer it so the server does not wait on us.
            (Some(id), Some(method)) => {
                let reply = match method {
                    "workspace/configuration" => {
                        let n = msg["params"]["items"].as_array().map_or(0, Vec::len);
                        json!({"jsonrpc": "2.0", "id": id, "result": vec![Value::Null; n]})
                    }
                    "client/registerCapability"
                    | "client/unregisterCapability"
                    | "window/workDoneProgress/create"
                    | "window/showMessageRequest"
                    | "workspace/workspaceFolders"
                    | "workspace/semanticTokens/refresh"
                    | "workspace/inlayHint/refresh"
                    | "workspace/codeLens/refresh"
                    | "workspace/diagnostic/refresh" => json!({"jsonrpc": "2.0", "id": id, "result": null}),
                    "window/showDocument" => json!({"jsonrpc": "2.0", "id": id, "result": {"success": false}}),
                    _ => json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "method not found"}}),
                };
                let _ = shared.send(&reply);
            }
            // A notification.
            (None, Some(method)) => match method {
                "textDocument/publishDiagnostics" => {
                    if let Some((path, diagnostics)) = types::parse_diagnostics(&msg["params"]) {
                        on_event(Event::Diagnostics { path, diagnostics });
                    }
                }
                "$/progress" => {
                    let token = msg["params"]["token"].to_string();
                    let v = &msg["params"]["value"];
                    match v["kind"].as_str() {
                        Some("begin") | Some("report") => {
                            progress.insert(token);
                            let mut text = v["title"].as_str().unwrap_or("").to_owned();
                            if let Some(m) = v["message"].as_str().filter(|m| !m.is_empty()) {
                                text = if text.is_empty() { m.to_owned() } else { format!("{text} {m}") };
                            }
                            if let Some(p) = v["percentage"].as_u64() {
                                text = format!("{text} {p}%");
                            }
                            on_event(Event::Progress(Some(text.trim().to_owned())));
                        }
                        Some("end") => {
                            progress.remove(&token);
                            if progress.is_empty() {
                                on_event(Event::Progress(None));
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            },
            // A response to one of our requests.
            (Some(id), None) => {
                let Some(id) = id.as_i64() else { continue };
                let cb = shared.pending.lock().unwrap().remove(&id);
                if let Some(cb) = cb {
                    match msg.get("error") {
                        Some(e) => cb(Err(e.get("message").and_then(Value::as_str).unwrap_or("server error").to_owned())),
                        None => cb(Ok(msg.get("result").cloned().unwrap_or(Value::Null))),
                    }
                }
            }
            (None, None) => {}
        }
    }
    shared.alive.store(false, Ordering::SeqCst);
    let pending: Vec<Callback> = shared.pending.lock().unwrap().drain().map(|(_, cb)| cb).collect();
    for cb in pending {
        cb(Err("the language server exited".into()));
    }
    on_event(Event::Exited);
}
