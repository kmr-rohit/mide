//! A small, UI-independent Language Server Protocol client.
//!
//! * [`frame`]: the `Content-Length` framing of JSON-RPC messages.
//! * [`pos`]: LSP counts columns in UTF-16 code units; mide counts chars. Conversions live here.
//! * [`types`]: diagnostics, locations, completion items and how to read them from JSON.
//! * [`servers`]: which server to run for which language, and where to find it.
//! * [`client`]: starts a server, keeps documents in sync, sends requests, delivers events.
//!
//! Nothing here knows about the editor, so it is tested with a mock server and, when it is
//! installed, a real one.

pub mod client;
pub mod frame;
pub mod pos;
pub mod servers;
pub mod types;

pub use client::{Client, Event};
pub use types::{CompletionItem, Diagnostic, Location, Range, Severity};
