//! Language servers for the open files: starting them off the UI thread, remembering what they
//! report, and handing their messages to the workspace through a channel.
//!
//! One server per (project folder, language family). Files of the same family in the same
//! project share it. Everything here is plain data and threads; the workspace owns the hub and
//! applies the messages on the UI thread.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use mide_lsp::servers::{self, Spec};
use mide_lsp::{Client, Diagnostic, Event};

/// A server is identified by the project folder it runs in and its language family.
pub type Key = (PathBuf, &'static str);

pub enum Msg {
    /// The start-up finished (the server is ready to use) or failed (a message for the user).
    Ready(Key, Result<Arc<Client>, String>),
    Event(Key, Event),
}

enum Slot {
    Starting,
    Ready(Arc<Client>),
    /// Starting failed or the server died; not retried until the folder is reopened.
    Failed,
}

pub struct Hub {
    tx: async_channel::Sender<Msg>,
    slots: HashMap<Key, Slot>,
    /// Latest diagnostics per file, kept so a tab opened later still gets them.
    diagnostics: HashMap<PathBuf, Vec<Diagnostic>>,
    /// What a server is busy with ("Indexing 40%"), if anything.
    pub progress: Option<(String, String)>,
}

impl Hub {
    pub fn new() -> (Hub, async_channel::Receiver<Msg>) {
        let (tx, rx) = async_channel::unbounded();
        (Hub { tx, slots: HashMap::new(), diagnostics: HashMap::new(), progress: None }, rx)
    }

    /// The server and project folder responsible for `file`, if its language has a server.
    pub fn key_for(file: &Path, workspace: Option<&Path>, language: &str) -> Option<(Key, Spec)> {
        let spec = servers::spec_for(language)?;
        let workspace = workspace.map(Path::to_path_buf).or_else(|| file.parent().map(Path::to_path_buf))?;
        Some(((servers::project_root(file, &workspace), spec.key), spec))
    }

    /// Forget every server (their processes stop when the last reference drops) and all diagnostics.
    pub fn reset(&mut self) {
        self.slots.clear();
        self.diagnostics.clear();
        self.progress = None;
    }

    pub fn client(&self, key: &Key) -> Option<Arc<Client>> {
        match self.slots.get(key) {
            Some(Slot::Ready(c)) if c.is_alive() => Some(c.clone()),
            _ => None,
        }
    }

    pub fn is_starting(&self, key: &Key) -> bool {
        matches!(self.slots.get(key), Some(Slot::Starting))
    }

    /// Start the server for `key` unless one is running, starting, or has already failed.
    pub fn ensure_started(&mut self, key: &Key, spec: &Spec) {
        if self.slots.contains_key(key) {
            return;
        }
        self.slots.insert(key.clone(), Slot::Starting);
        let (tx, key, spec) = (self.tx.clone(), key.clone(), spec.clone());
        std::thread::spawn(move || {
            let result = match servers::resolve(&spec) {
                None => Err(format!("No language server for {}: {}", spec.key, spec.hint)),
                Some((program, args)) => {
                    let (events, ev_key) = (tx.clone(), key.clone());
                    Client::start(&program, args, &key.0, &servers::extended_path(), Duration::from_secs(60), move |e| {
                        let _ = events.send_blocking(Msg::Event(ev_key.clone(), e));
                    })
                    .map(Arc::new)
                    .map_err(|e| {
                        let name = program.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        format!("{name} did not start ({e}). To install it: {}", spec.hint)
                    })
                }
            };
            let _ = tx.send_blocking(Msg::Ready(key, result));
        });
    }

    /// Apply a start-up result. Returns a message to show the user when it failed.
    pub fn ready(&mut self, key: Key, result: Result<Arc<Client>, String>) -> Option<String> {
        match result {
            Ok(client) => {
                self.slots.insert(key, Slot::Ready(client));
                None
            }
            Err(msg) => {
                self.slots.insert(key, Slot::Failed);
                Some(msg)
            }
        }
    }

    /// Apply a server message. Returns the file whose diagnostics changed, if any.
    pub fn event(&mut self, key: &Key, ev: Event) -> Option<PathBuf> {
        match ev {
            Event::Diagnostics { path, diagnostics } => {
                if diagnostics.is_empty() {
                    self.diagnostics.remove(&path);
                } else {
                    self.diagnostics.insert(path.clone(), diagnostics);
                }
                Some(path)
            }
            Event::Progress(p) => {
                self.progress = p.map(|text| (key.1.to_owned(), text));
                None
            }
            Event::Exited => {
                self.slots.insert(key.clone(), Slot::Failed);
                self.progress = None;
                None
            }
        }
    }

    pub fn diagnostics_for(&self, path: &Path) -> Vec<Diagnostic> {
        self.diagnostics.get(path).cloned().unwrap_or_default()
    }

    /// Forget a file's diagnostics (its tab closed).
    pub fn forget(&mut self, path: &Path) {
        self.diagnostics.remove(path);
    }

    /// (errors, warnings) across every file the servers have reported on.
    pub fn totals(&self) -> (usize, usize) {
        use mide_lsp::Severity;
        let count = |sev| self.diagnostics.values().flatten().filter(|d| d.severity == sev).count();
        (count(Severity::Error), count(Severity::Warning))
    }
}

/// "✗2 ⚠1", or nothing when the count is zero.
pub fn summary((errors, warnings): (usize, usize)) -> String {
    match (errors, warnings) {
        (0, 0) => String::new(),
        (e, 0) => format!("✗{e}"),
        (0, w) => format!("⚠{w}"),
        (e, w) => format!("✗{e} ⚠{w}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mide_lsp::types::{Position, Range};
    use mide_lsp::Severity;

    fn diag(sev: Severity) -> Diagnostic {
        Diagnostic { range: Range { start: Position::default(), end: Position { line: 0, character: 1 } }, severity: sev, message: "m".into(), source: None, code: None }
    }

    #[test]
    fn summary_text() {
        assert_eq!(summary((0, 0)), "");
        assert_eq!(summary((2, 0)), "✗2");
        assert_eq!(summary((0, 3)), "⚠3");
        assert_eq!(summary((2, 3)), "✗2 ⚠3");
    }

    #[test]
    fn diagnostics_are_kept_per_file_and_cleared_by_an_empty_list() {
        let (mut hub, _rx) = Hub::new();
        let key: Key = (PathBuf::from("/p"), "rust");
        let a = PathBuf::from("/p/a.rs");
        let ev = |ds| Event::Diagnostics { path: PathBuf::from("/p/a.rs"), diagnostics: ds };
        assert_eq!(hub.event(&key, ev(vec![diag(Severity::Error), diag(Severity::Warning)])), Some(a.clone()));
        assert_eq!(hub.diagnostics_for(&a).len(), 2);
        assert_eq!(hub.totals(), (1, 1));
        hub.event(&key, ev(vec![]));
        assert!(hub.diagnostics_for(&a).is_empty());
        assert_eq!(hub.totals(), (0, 0));
    }

    #[test]
    fn progress_is_tagged_with_the_server_and_cleared() {
        let (mut hub, _rx) = Hub::new();
        let key: Key = (PathBuf::from("/p"), "rust");
        hub.event(&key, Event::Progress(Some("Indexing 40%".into())));
        assert_eq!(hub.progress, Some(("rust".into(), "Indexing 40%".into())));
        hub.event(&key, Event::Progress(None));
        assert!(hub.progress.is_none());
    }

    #[test]
    fn a_failed_or_dead_server_is_not_restarted_and_has_no_client() {
        let (mut hub, _rx) = Hub::new();
        let key: Key = (PathBuf::from("/p"), "rust");
        let msg = hub.ready(key.clone(), Err("rust-analyzer did not start".into()));
        assert_eq!(msg.as_deref(), Some("rust-analyzer did not start"));
        assert!(hub.client(&key).is_none());
        assert!(!hub.is_starting(&key));
        // ensure_started is a no-op for a failed slot (no thread, no message).
        let spec = servers::spec_for("Rust").unwrap();
        hub.ensure_started(&key, &spec);
        assert!(!hub.is_starting(&key));
    }

    #[test]
    fn key_picks_the_project_folder_and_the_family() {
        assert!(Hub::key_for(Path::new("/p/a.txt"), Some(Path::new("/p")), "Plain Text").is_none());
        let (key, spec) = Hub::key_for(Path::new("/p/src/a.ts"), Some(Path::new("/p")), "TypeScript").unwrap();
        assert_eq!(key, (PathBuf::from("/p"), "typescript"));
        assert_eq!(spec.language_id, "typescript");
    }
}
