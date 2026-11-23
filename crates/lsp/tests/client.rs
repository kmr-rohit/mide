//! The client against a mock server (always) and against rust-analyzer (`--ignored`, needs it installed).

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::Mutex;
use std::time::Duration;

use mide_lsp::servers;
use mide_lsp::types::Position;
use mide_lsp::{Client, Event, Severity};

const T: Duration = Duration::from_secs(10);

fn start_mock(root: &Path) -> (Client, Receiver<Event>) {
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/mock_server.py");
    let script = script.to_string_lossy().into_owned();
    let client = Client::start(Path::new("python3"), &["-u", &script], root, &servers::extended_path(), T, move |e| {
        let _ = tx.lock().unwrap().send(e);
    })
    .expect("mock server starts");
    (client, rx)
}

fn next_diagnostics(rx: &Receiver<Event>) -> (PathBuf, Vec<mide_lsp::Diagnostic>) {
    loop {
        match rx.recv_timeout(T).expect("an event arrives") {
            Event::Diagnostics { path, diagnostics } => return (path, diagnostics),
            _ => continue,
        }
    }
}

#[test]
fn handshake_documents_diagnostics_and_features() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.txt");
    let (client, rx) = start_mock(dir.path());
    assert!(client.supports("definitionProvider"));
    assert!(!client.supports("renameProvider"));

    // Progress from the server is reported, then cleared.
    let mut saw_begin = false;
    loop {
        match rx.recv_timeout(T).unwrap() {
            Event::Progress(Some(t)) => {
                assert_eq!(t, "Indexing 10%");
                saw_begin = true;
            }
            Event::Progress(None) => break,
            _ => {}
        }
    }
    assert!(saw_begin);

    // Open: the error line gets a diagnostic; fixing the text clears it.
    client.did_open(&file, "plain", "fine\nthis is an ERROR\n");
    assert!(client.is_open(&file));
    let (path, d) = next_diagnostics(&rx);
    assert_eq!(path, file);
    assert_eq!(d.len(), 1);
    assert_eq!((d[0].range.start.line, d[0].range.start.character, d[0].severity), (1, 11, Severity::Error));
    assert_eq!(d[0].source.as_deref(), Some("mock"));
    client.did_change(&file, "all fine now\n");
    assert!(next_diagnostics(&rx).1.is_empty());

    // Requests.
    let (tx, rrx) = mpsc::channel();
    client.definition(&file, Position { line: 0, character: 0 }, move |r| tx.send(r).unwrap());
    let locs = rrx.recv_timeout(T).unwrap().unwrap();
    assert_eq!(locs[0].path, file);
    assert_eq!(locs[0].range.start.character, 1);

    let (tx, rrx) = mpsc::channel();
    client.completion(&file, Position::default(), move |r| tx.send(r).unwrap());
    let items = rrx.recv_timeout(T).unwrap().unwrap();
    assert_eq!(items.iter().map(|i| i.insert_text.as_str()).collect::<Vec<_>>(), ["alpha", "beta()"]);

    // The server's own request (workspace/configuration) was answered with one null per item.
    let (tx, rrx) = mpsc::channel();
    client.hover(&file, Position::default(), move |r| tx.send(r).unwrap());
    assert_eq!(rrx.recv_timeout(T).unwrap().unwrap().as_deref(), Some("config reply: [null, null]"));

    // A method the server does not know is an error, not a hang.
    assert!(client.request_blocking("textDocument/rename", serde_json::json!({}), T).unwrap_err().contains("unknown"));

    client.did_close(&file);
    assert!(!client.is_open(&file));
    client.shutdown();
}

#[test]
fn a_crashing_server_fails_pending_requests_and_reports_exit() {
    let dir = tempfile::tempdir().unwrap();
    let (client, rx) = start_mock(dir.path());
    let (tx, rrx) = mpsc::channel();
    client.request("x/die", serde_json::Value::Null, move |r| tx.send(r).unwrap());
    let err = rrx.recv_timeout(T).unwrap().unwrap_err();
    assert!(err.contains("exited"), "{err}");
    loop {
        if rx.recv_timeout(T).unwrap() == Event::Exited {
            break;
        }
    }
    assert!(!client.is_alive());
    // Further requests fail at once instead of waiting.
    assert!(client.request_blocking("textDocument/hover", serde_json::Value::Null, T).is_err());
}

#[test]
fn a_missing_program_is_a_readable_error() {
    let dir = tempfile::tempdir().unwrap();
    let err = Client::start(Path::new("/definitely/not/here"), &[], dir.path(), "".as_ref(), T, |_| {}).err().unwrap();
    assert!(err.contains("could not start"), "{err}");
}

/// The real thing: `cargo test -p mide-lsp -- --ignored` (needs rust-analyzer; takes a while the first time).
#[test]
#[ignore]
fn rust_analyzer_reports_a_type_error_and_finds_a_definition() {
    let spec = servers::spec_for("Rust").unwrap();
    let Some((program, args)) = servers::resolve(&spec) else {
        eprintln!("rust-analyzer is not installed; skipping");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    let main = dir.path().join("src/main.rs");
    let text = "fn helper() -> u32 { 1 }\nfn main() {\n    let x: i32 = \"not a number\";\n    let _ = helper();\n}\n";
    std::fs::write(&main, text).unwrap();
    let root = dir.path().canonicalize().unwrap();
    let main = root.join("src/main.rs");

    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    let client = Client::start(&program, args, &root, &servers::extended_path(), Duration::from_secs(60), move |e| {
        let _ = tx.lock().unwrap().send(e);
    })
    .expect("rust-analyzer starts");
    client.did_open(&main, "rust", text);

    let deadline = std::time::Instant::now() + Duration::from_secs(90);
    let mut found = false;
    while std::time::Instant::now() < deadline {
        if let Ok(Event::Diagnostics { path, diagnostics }) = rx.recv_timeout(Duration::from_secs(5)) {
            if path == main && diagnostics.iter().any(|d| d.severity == Severity::Error && d.message.contains("mismatched types")) {
                found = true;
                break;
            }
        }
    }
    assert!(found, "no 'mismatched types' error arrived");

    // Go to definition on `helper` in `let _ = helper();` (line 3, column 12).
    let (tx, rrx) = mpsc::channel();
    client.definition(&main, Position { line: 3, character: 13 }, move |r| tx.send(r).unwrap());
    let locs = rrx.recv_timeout(Duration::from_secs(30)).unwrap().unwrap();
    assert_eq!(locs[0].path, main);
    assert_eq!(locs[0].range.start.line, 0, "helper is defined on the first line");
    client.shutdown();
}
