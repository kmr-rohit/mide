#!/usr/bin/env python3
"""A tiny language server for tests: diagnostics for lines containing ERROR, canned answers."""
import json, sys

def read():
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.strip()
        if not line:
            break
        k, v = line.decode().split(":", 1)
        headers[k.lower()] = v.strip()
    return json.loads(sys.stdin.buffer.read(int(headers["content-length"])))

def send(m):
    body = json.dumps(m).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
    sys.stdout.buffer.flush()

def diagnostics(uri, text):
    out = []
    for i, line in enumerate(text.split("\n")):
        col = line.find("ERROR")
        if col >= 0:
            out.append({"range": {"start": {"line": i, "character": col}, "end": {"line": i, "character": col + 5}},
                        "severity": 1, "message": "found ERROR", "source": "mock"})
    send({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {"uri": uri, "diagnostics": out}})

config_reply = None
while True:
    m = read()
    if m is None:
        break
    method, id_ = m.get("method"), m.get("id")
    if method == "initialize":
        # A request the client must answer before we carry on.
        send({"jsonrpc": "2.0", "id": 900, "method": "workspace/configuration", "params": {"items": [{}, {}]}})
        send({"jsonrpc": "2.0", "id": id_, "result": {"capabilities": {"definitionProvider": True, "hoverProvider": True, "textDocumentSync": 1}}})
    elif method == "initialized":
        send({"jsonrpc": "2.0", "method": "$/progress", "params": {"token": "t", "value": {"kind": "begin", "title": "Indexing", "percentage": 10}}})
        send({"jsonrpc": "2.0", "method": "$/progress", "params": {"token": "t", "value": {"kind": "end"}}})
    elif method in ("textDocument/didOpen", "textDocument/didChange"):
        td = m["params"]["textDocument"]
        text = td["text"] if method == "textDocument/didOpen" else m["params"]["contentChanges"][-1]["text"]
        diagnostics(td["uri"], text)
    elif method == "textDocument/definition":
        send({"jsonrpc": "2.0", "id": id_, "result": {"uri": m["params"]["textDocument"]["uri"],
              "range": {"start": {"line": 0, "character": 1}, "end": {"line": 0, "character": 3}}}})
    elif method == "textDocument/completion":
        send({"jsonrpc": "2.0", "id": id_, "result": {"isIncomplete": False, "items": [{"label": "alpha"}, {"label": "beta", "insertText": "beta()"}]}})
    elif method == "textDocument/hover":
        send({"jsonrpc": "2.0", "id": id_, "result": {"contents": {"kind": "plaintext", "value": "config reply: %s" % json.dumps(config_reply)}}})
    elif method == "shutdown":
        send({"jsonrpc": "2.0", "id": id_, "result": None})
    elif method == "exit":
        break
    elif method == "x/die":
        sys.exit(3)
    elif id_ == 900 and method is None:
        config_reply = m.get("result")
    elif id_ is not None and method is not None:
        send({"jsonrpc": "2.0", "id": id_, "error": {"code": -32601, "message": "unknown " + method}})
