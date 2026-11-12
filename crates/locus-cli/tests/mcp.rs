// SPDX-License-Identifier: AGPL-3.0-or-later

//! SPEC §16.5: drive `locus-cli mcp` over stdio with scripted tool calls and
//! compare the saved document against a golden file (#228). Regenerate the
//! golden with `LOCUS_UPDATE_GOLDEN=1`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{Value, json};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("locus-cli-mcp-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    let _ = std::fs::remove_file(&p);
    p
}

/// A JSON-RPC client over the child's stdin/stdout, one message per line.
struct Client {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next: u64,
}

impl Client {
    fn start(doc: &Path, scopes: &str) -> Client {
        let mut child = Command::new(env!("CARGO_BIN_EXE_locus-cli"))
            .args(["mcp", "--doc"])
            .arg(doc)
            .args(["--scopes", scopes])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Client {
            child,
            stdin,
            stdout,
            next: 1,
        }
    }

    fn send(&mut self, msg: &Value) {
        writeln!(self.stdin, "{msg}").unwrap();
        self.stdin.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let mut line = String::new();
            assert!(
                self.stdout.read_line(&mut line).unwrap() > 0,
                "server closed"
            );
            let msg: Value = serde_json::from_str(&line).unwrap();
            if msg["id"] == id {
                return msg;
            }
        }
    }

    /// The 2025-11-25 handshake (rmcp's legacy path, ADR 0005).
    fn initialize(&mut self) -> Value {
        let r = self.request(
            "initialize",
            json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "locus-test", "version": "0"}
            }),
        );
        self.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        r
    }

    /// `tools/call`, returning the result object.
    fn call(&mut self, name: &str, args: Value) -> Value {
        let r = self.request("tools/call", json!({"name": name, "arguments": args}));
        assert!(r.get("error").is_none(), "{name}: {r}");
        r["result"].clone()
    }

    fn ok(&mut self, name: &str, args: Value) -> Value {
        let r = self.call(name, args);
        assert_eq!(r["isError"], false, "{name}: {r}");
        r["structuredContent"].clone()
    }

    fn close(mut self) {
        drop(self.stdin);
        let status = self.child.wait().unwrap();
        assert!(status.success());
    }
}

fn golden(name: &str, got: &str) {
    let path = fixture("mcp").join(name);
    if std::env::var_os("LOCUS_UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, got).unwrap();
    }
    let want = std::fs::read_to_string(&path)
        .unwrap()
        .replace("\r\n", "\n");
    assert_eq!(got.replace("\r\n", "\n"), want, "differs from {name}");
}

#[test]
fn scripted_session_matches_golden_document() {
    let doc = scratch("bracket.locus");
    let mut c = Client::start(&doc, "read,write,export");
    let init = c.initialize();
    assert_eq!(init["result"]["serverInfo"]["name"], "locus");
    assert_eq!(init["result"]["protocolVersion"], "2025-11-25");

    let tools = c.request("tools/list", json!({}));
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"add_entities") && names.contains(&"save"));

    c.ok("set_parameter", json!({"name": "w", "expr": "30"}));
    let r = c.ok(
        "add_entities",
        json!({
            "entities": [
                {"name": "p0", "type": "point", "x": 0.0, "y": 0.0},
                {"name": "p1", "type": "point", "x": 30.0, "y": 0.0},
                {"name": "p2", "type": "point", "x": 30.0, "y": 20.0},
                {"name": "p3", "type": "point", "x": 0.0, "y": 20.0},
                {"name": "bottom", "type": "line", "start": "p0", "end": "p1"},
                {"name": "right", "type": "line", "start": "p1", "end": "p2"},
                {"name": "top", "type": "line", "start": "p2", "end": "p3"},
                {"name": "left", "type": "line", "start": "p3", "end": "p0"}
            ],
            "constraints": [
                {"type": "fix", "point": "p0"},
                {"type": "horizontal", "line": "bottom"},
                {"type": "vertical", "line": "right"},
                {"type": "horizontal", "line": "top"},
                {"type": "vertical", "line": "left"},
                {"type": "distance", "a": "p0", "b": "p1", "expr": "w"},
                {"type": "distance", "a": "p1", "b": "p2", "value": 20.0}
            ]
        }),
    );
    assert_eq!(r["undo_label"], "AI: add 8 entities, 7 constraints");
    assert_eq!(r["solve"]["dof"], 0);

    let layer = c.ok(
        "create_layer",
        json!({"name": "Holes", "colour": [255, 0, 0], "operation": "cut"}),
    )["created"]["layers"][0]
        .clone();
    c.ok(
        "add_entities",
        json!({
            "layer": layer,
            "entities": [{"name": "hole", "type": "circle", "center": [15.0, 10.0], "radius": 3.0}],
            "constraints": [{"type": "diameter", "curve": "hole", "value": 6.0}]
        }),
    );
    let summary = c.ok("get_document_summary", json!({}));
    assert_eq!(summary["entities"], 10);
    assert_eq!(summary["constraints"], 8);

    // Export over stdio returns the bytes, never a file (§16.4).
    let svg = c.call("export", json!({"format": "svg"}));
    assert_eq!(svg["isError"], false);
    let text = svg["content"][1]["resource"]["text"].as_str().unwrap();
    assert!(text.starts_with("<?xml") || text.starts_with("<svg"));

    c.ok("save", json!({}));
    c.close();
    golden("bracket.locus", &std::fs::read_to_string(&doc).unwrap());
}

#[test]
fn read_only_by_default_over_stdio() {
    let doc = scratch("plate.locus");
    std::fs::copy(fixture("plate.locus"), &doc).unwrap();
    let before = std::fs::read(&doc).unwrap();
    let mut c = Client::start(&doc, "read");
    c.initialize();
    let r = c.call(
        "add_entities",
        json!({"entities": [{"type": "point", "x": 1.0, "y": 1.0}]}),
    );
    assert_eq!(r["isError"], true);
    assert_eq!(r["structuredContent"]["error"]["kind"], "scope_denied");
    let r = c.call("export", json!({"format": "svg"}));
    assert_eq!(r["structuredContent"]["error"]["scope"], "export");
    // No write scope, no save tool.
    let r = c.request("tools/call", json!({"name": "save", "arguments": {}}));
    assert!(r.get("error").is_some());
    c.ok("get_document_summary", json!({}));
    c.close();
    assert_eq!(std::fs::read(&doc).unwrap(), before, "file untouched");
}

#[test]
fn stateless_2026_07_28_requests() {
    let doc = scratch("stateless.locus");
    std::fs::copy(fixture("plate.locus"), &doc).unwrap();
    let mut c = Client::start(&doc, "read");
    let meta = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": {"name": "locus-test", "version": "0"}
    });
    let r = c.request(
        "tools/call",
        json!({"name": "get_document_summary", "arguments": {}, "_meta": meta}),
    );
    assert_eq!(r["result"]["isError"], false, "{r}");
    assert_eq!(r["result"]["structuredContent"]["entities"], 11);
    let r = c.request(
        "resources/read",
        json!({"uri": "locus://layers", "_meta": meta}),
    );
    let layers: Value =
        serde_json::from_str(r["result"]["contents"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(layers[0]["name"], "Layer 1");
    c.close();
}

#[test]
fn bad_scopes_are_a_usage_error() {
    let out = Command::new(env!("CARGO_BIN_EXE_locus-cli"))
        .args(["mcp", "--doc", "x.locus", "--scopes", "read,admin"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}
