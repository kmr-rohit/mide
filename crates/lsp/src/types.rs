//! The few LSP shapes mide uses, read from JSON by hand (no code-generated protocol crate).
//! Positions stay in the protocol's UTF-16 columns; convert with [`crate::pos`] at the edge.

use std::path::{Path, PathBuf};

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Position {
    pub line: u32,
    /// UTF-16 code units.
    pub character: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Info,
    Hint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub range: Range,
    pub severity: Severity,
    pub message: String,
    pub source: Option<String>,
    pub code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub path: PathBuf,
    pub range: Range,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: String,
    /// LSP `CompletionItemKind` number, if the server sent one.
    pub kind: Option<u32>,
    pub detail: Option<String>,
    /// What to insert: the edit's text, else `insertText`, else the label.
    pub insert_text: String,
    /// The range the server wants replaced, if it said so.
    pub replace: Option<Range>,
    pub sort_key: String,
}

// ---- file:// URIs --------------------------------------------------------------------------------

pub fn uri_from_path(path: &Path) -> String {
    let mut out = String::from("file://");
    for b in path.to_string_lossy().bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn path_from_uri(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok().map(PathBuf::from)
}

// ---- reading results -----------------------------------------------------------------------------

fn u32_at(v: &Value, key: &str) -> u32 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0).min(u32::MAX as u64) as u32
}

fn position(v: &Value) -> Position {
    Position { line: u32_at(v, "line"), character: u32_at(v, "character") }
}

fn range(v: &Value) -> Range {
    Range { start: position(&v["start"]), end: position(&v["end"]) }
}

/// `textDocument/publishDiagnostics` params: the file and its diagnostics (an empty list clears them).
pub fn parse_diagnostics(params: &Value) -> Option<(PathBuf, Vec<Diagnostic>)> {
    let path = path_from_uri(params.get("uri")?.as_str()?)?;
    let list = params.get("diagnostics")?.as_array()?;
    let out = list
        .iter()
        .map(|d| Diagnostic {
            range: range(&d["range"]),
            severity: match d.get("severity").and_then(Value::as_u64) {
                Some(2) => Severity::Warning,
                Some(3) => Severity::Info,
                Some(4) => Severity::Hint,
                _ => Severity::Error,
            },
            message: d.get("message").and_then(Value::as_str).unwrap_or("").to_owned(),
            source: d.get("source").and_then(Value::as_str).map(str::to_owned),
            code: d.get("code").map(|c| c.as_str().map(str::to_owned).unwrap_or_else(|| c.to_string())),
        })
        .collect();
    Some((path, out))
}

/// The result of `textDocument/definition` (and friends): a `Location`, a list of them, or `LocationLink`s.
pub fn parse_locations(v: &Value) -> Vec<Location> {
    let one = |x: &Value| -> Option<Location> {
        // Location { uri, range } or LocationLink { targetUri, targetSelectionRange }
        let (uri, r) = match x.get("targetUri") {
            Some(u) => (u.as_str()?, x.get("targetSelectionRange").or_else(|| x.get("targetRange"))?),
            None => (x.get("uri")?.as_str()?, x.get("range")?),
        };
        Some(Location { path: path_from_uri(uri)?, range: range(r) })
    };
    match v {
        Value::Array(a) => a.iter().filter_map(one).collect(),
        Value::Object(_) => one(v).into_iter().collect(),
        _ => Vec::new(),
    }
}

/// `textDocument/completion`: a list or `{ items: [...] }`.
pub fn parse_completion(v: &Value) -> Vec<CompletionItem> {
    let items = match v {
        Value::Array(a) => a.as_slice(),
        Value::Object(_) => v.get("items").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]),
        _ => &[],
    };
    items
        .iter()
        .filter_map(|i| {
            let label = i.get("label")?.as_str()?.to_owned();
            let edit = i.get("textEdit");
            // textEdit is a TextEdit { range, newText } or an InsertReplaceEdit { insert, replace, newText }.
            let replace = edit.and_then(|e| e.get("range").or_else(|| e.get("replace"))).map(range);
            let insert_text = edit
                .and_then(|e| e.get("newText"))
                .and_then(Value::as_str)
                .or_else(|| i.get("insertText").and_then(Value::as_str))
                .unwrap_or(&label)
                .to_owned();
            Some(CompletionItem {
                kind: i.get("kind").and_then(Value::as_u64).map(|k| k as u32),
                detail: i.get("detail").and_then(Value::as_str).map(str::to_owned),
                sort_key: i.get("sortText").and_then(Value::as_str).unwrap_or(&label).to_owned(),
                insert_text,
                replace,
                label,
            })
        })
        .collect()
}

/// `textDocument/hover`: plain text from a string, `{ kind, value }`, or a list of those.
pub fn parse_hover(v: &Value) -> Option<String> {
    fn text(c: &Value) -> Option<String> {
        match c {
            Value::String(s) => Some(s.clone()),
            Value::Array(a) => {
                let parts: Vec<String> = a.iter().filter_map(text).collect();
                (!parts.is_empty()).then(|| parts.join("\n\n"))
            }
            Value::Object(_) => c.get("value").and_then(Value::as_str).map(str::to_owned),
            _ => None,
        }
    }
    text(v.get("contents")?).filter(|s| !s.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn uris_round_trip_spaces_and_unicode() {
        let p = Path::new("/Users/me/My Project/é→x#1.rs");
        let uri = uri_from_path(p);
        assert_eq!(uri, "file:///Users/me/My%20Project/%C3%A9%E2%86%92x%231.rs");
        assert_eq!(path_from_uri(&uri).unwrap(), p);
        assert_eq!(path_from_uri("file:///a/b").unwrap(), Path::new("/a/b"));
        assert!(path_from_uri("https://x").is_none());
        assert!(path_from_uri("file:///bad%2").is_some(), "a dangling % is kept literally");
    }

    #[test]
    fn diagnostics_are_read_with_defaults() {
        let (path, d) = parse_diagnostics(&json!({
            "uri": "file:///p/a.rs",
            "diagnostics": [
                {"range": {"start": {"line": 1, "character": 2}, "end": {"line": 1, "character": 5}},
                 "severity": 2, "message": "unused", "source": "rustc", "code": "unused_variables"},
                {"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}, "message": "boom", "code": 7}
            ]
        }))
        .unwrap();
        assert_eq!(path, Path::new("/p/a.rs"));
        assert_eq!(d[0].severity, Severity::Warning);
        assert_eq!(d[0].range.start, Position { line: 1, character: 2 });
        assert_eq!(d[0].code.as_deref(), Some("unused_variables"));
        assert_eq!(d[1].severity, Severity::Error, "no severity means error");
        assert_eq!(d[1].code.as_deref(), Some("7"));
        assert!(parse_diagnostics(&json!({"uri": "file:///p/a.rs", "diagnostics": []})).unwrap().1.is_empty());
    }

    #[test]
    fn locations_accept_every_shape() {
        let loc = json!({"uri": "file:///a.rs", "range": {"start": {"line": 3, "character": 4}, "end": {"line": 3, "character": 9}}});
        assert_eq!(parse_locations(&loc).len(), 1);
        assert_eq!(parse_locations(&json!([loc.clone(), loc.clone()])).len(), 2);
        let link = json!([{"targetUri": "file:///b.rs", "targetRange": {"start": {"line": 0, "character": 0}, "end": {"line": 9, "character": 0}},
                           "targetSelectionRange": {"start": {"line": 2, "character": 1}, "end": {"line": 2, "character": 4}}}]);
        let l = parse_locations(&link);
        assert_eq!(l[0].path, Path::new("/b.rs"));
        assert_eq!(l[0].range.start.line, 2, "the selection range wins over the whole body");
        assert!(parse_locations(&Value::Null).is_empty());
    }

    #[test]
    fn completion_items_prefer_the_text_edit() {
        let v = json!({"isIncomplete": false, "items": [
            {"label": "println!", "kind": 3, "detail": "macro", "sortText": "1", "insertText": "println!($0)",
             "textEdit": {"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 3}}, "newText": "println!"}},
            {"label": "len", "insertText": "len()"},
            {"label": "x"},
            {"nolabel": true}
        ]});
        let items = parse_completion(&v);
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].insert_text, "println!");
        assert_eq!(items[0].replace.unwrap().end.character, 3);
        assert_eq!(items[1].insert_text, "len()");
        assert_eq!(items[2].insert_text, "x");
        assert_eq!(parse_completion(&json!([{"label": "a"}])).len(), 1);
    }

    #[test]
    fn hover_text_comes_from_any_content_shape() {
        assert_eq!(parse_hover(&json!({"contents": "plain"})).as_deref(), Some("plain"));
        assert_eq!(parse_hover(&json!({"contents": {"kind": "markdown", "value": "**x**"}})).as_deref(), Some("**x**"));
        assert_eq!(parse_hover(&json!({"contents": ["a", {"language": "rust", "value": "b"}]})).as_deref(), Some("a\n\nb"));
        assert!(parse_hover(&json!({"contents": "  "})).is_none());
        assert!(parse_hover(&Value::Null).is_none());
    }
}
