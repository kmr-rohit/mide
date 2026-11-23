//! `Content-Length: N\r\n\r\n<N bytes of JSON>` messages.

use std::io::{self, BufRead, Write};

use serde_json::Value;

/// Refuse absurd messages instead of allocating them.
const MAX_BODY: usize = 256 << 20;

pub fn write_message(w: &mut impl Write, msg: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(msg).map_err(io::Error::other)?;
    write!(w, "Content-Length: {}\r\n\r\n", body.len())?;
    w.write_all(&body)?;
    w.flush()
}

/// The next message, or `None` at end of stream.
pub fn read_message(r: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut len: Option<usize> = None;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            if len.is_some() {
                break;
            }
            continue; // stray blank line before any header
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                len = Some(value.trim().parse().map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad Content-Length"))?);
            }
        }
    }
    let len = len.unwrap_or(0);
    if len > MAX_BODY {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "message too large"));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    serde_json::from_slice(&body).map(Some).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn round_trips_several_messages_including_unicode() {
        let mut buf = Vec::new();
        write_message(&mut buf, &json!({"jsonrpc": "2.0", "id": 1, "method": "x", "params": {"s": "é→😀"}})).unwrap();
        write_message(&mut buf, &json!({"jsonrpc": "2.0", "method": "y"})).unwrap();
        let mut r = io::Cursor::new(buf);
        let a = read_message(&mut r).unwrap().unwrap();
        assert_eq!(a["params"]["s"], "é→😀");
        assert_eq!(read_message(&mut r).unwrap().unwrap()["method"], "y");
        assert!(read_message(&mut r).unwrap().is_none());
    }

    #[test]
    fn extra_headers_and_case_are_tolerated() {
        let body = r#"{"a":1}"#;
        let raw = format!("content-type: application/vscode-jsonrpc; charset=utf-8\r\nCONTENT-LENGTH: {}\r\n\r\n{body}", body.len());
        let v = read_message(&mut io::Cursor::new(raw.into_bytes())).unwrap().unwrap();
        assert_eq!(v["a"], 1);
    }

    #[test]
    fn bad_input_is_an_error_not_a_hang() {
        assert!(read_message(&mut io::Cursor::new(b"Content-Length: nope\r\n\r\n".to_vec())).is_err());
        assert!(read_message(&mut io::Cursor::new(b"Content-Length: 10\r\n\r\nshort".to_vec())).is_err());
        assert!(read_message(&mut io::Cursor::new(b"Content-Length: 99999999999\r\n\r\n".to_vec())).is_err());
        assert!(read_message(&mut io::Cursor::new(Vec::new())).unwrap().is_none());
    }
}
