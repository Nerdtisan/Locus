// SPDX-License-Identifier: AGPL-3.0-or-later

//! SPEC §16.5 security tests for the localhost HTTP transport (#229):
//! non-loopback bind, missing/invalid token, bad `Origin`, bad `Host`,
//! payload cap, scope enforcement, and a working session.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use locus_mcp::http::{self, HttpError};
use locus_mcp::{Grants, Headless, LocalBackend, LocusServer, NeverApprove, Options};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const TOKEN: &str = "test-token-0123456789abcdefghijklmnopqrstuv";

async fn start(grants: Grants) -> http::Running {
    let backend = LocalBackend::spawn(Headless::new(locus_doc::Document::new())).unwrap();
    let server = LocusServer::new(backend, Arc::new(NeverApprove), grants, Options::default());
    http::start("127.0.0.1:0".parse().unwrap(), server, TOKEN.into())
        .await
        .unwrap()
}

struct Reply {
    status: u16,
    body: String,
}

impl Reply {
    /// The JSON-RPC message in a JSON or SSE body.
    fn message(&self) -> Value {
        let data = self
            .body
            .lines()
            .filter_map(|l| l.strip_prefix("data:"))
            .map(str::trim)
            .find(|d| d.starts_with('{'))
            .unwrap_or(self.body.trim());
        serde_json::from_str(data).unwrap_or_else(|_| panic!("not JSON: {}", self.body))
    }
}

/// One HTTP/1.1 POST; `headers` are added as given.
async fn post(addr: SocketAddr, headers: &[(&str, &str)], body: &str) -> Reply {
    let mut s = TcpStream::connect(addr).await.unwrap();
    let mut req = format!(
        "POST {} HTTP/1.1\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n",
        http::PATH,
        body.len()
    );
    let has_host = headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("host"));
    if !has_host {
        req += &format!("Host: {addr}\r\n");
    }
    for (k, v) in headers {
        req += &format!("{k}: {v}\r\n");
    }
    req += "\r\n";
    s.write_all(req.as_bytes()).await.unwrap();
    s.write_all(body.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), s.read_to_end(&mut out)).await;
    let text = String::from_utf8_lossy(&out).into_owned();
    let status = text
        .split(' ')
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let body = text
        .split_once("\r\n\r\n")
        .map(|x| x.1)
        .unwrap_or("")
        .to_owned();
    Reply { status, body }
}

fn bearer() -> String {
    format!("Bearer {TOKEN}")
}

/// A stateless 2026-07-28 `tools/call`.
fn call(name: &str, args: Value) -> String {
    json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {
            "name": name, "arguments": args,
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {},
                "io.modelcontextprotocol/clientInfo": {"name": "t", "version": "0"}
            }
        }
    })
    .to_string()
}

const V: (&str, &str) = ("MCP-Protocol-Version", "2026-07-28");

/// The standard request headers of 2026-07-28 (SEP-2243), for `tools/call`.
async fn post_call(addr: SocketAddr, extra: &[(&str, &str)], name: &str, args: Value) -> Reply {
    let mut h = vec![V, ("Mcp-Method", "tools/call"), ("Mcp-Name", name)];
    h.extend_from_slice(extra);
    post(addr, &h, &call(name, args)).await
}

#[test]
fn non_loopback_bind_is_rejected() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        for addr in ["0.0.0.0:0", "[::]:0", "10.0.0.1:0"] {
            let backend = LocalBackend::spawn(Headless::new(locus_doc::Document::new())).unwrap();
            let server = LocusServer::new(
                backend,
                Arc::new(NeverApprove),
                Grants::default(),
                Options::default(),
            );
            let r = http::start(addr.parse().unwrap(), server, TOKEN.into()).await;
            assert!(matches!(r, Err(HttpError::NotLoopback(_))), "{addr}");
        }
    });
}

#[tokio::test]
async fn missing_or_invalid_token_is_rejected() {
    let s = start(Grants::default()).await;
    let summary = |extra: &'static [(&'static str, &'static str)]| {
        post_call(s.addr, extra, "get_document_summary", json!({}))
    };
    assert_eq!(summary(&[]).await.status, 401);
    assert_eq!(
        summary(&[("Authorization", "Bearer wrong")]).await.status,
        401
    );
    assert_eq!(
        summary(&[("Authorization", TOKEN)]).await.status,
        401,
        "the Bearer scheme is required"
    );
    let auth = bearer();
    let r = post_call(
        s.addr,
        &[("Authorization", &auth)],
        "get_document_summary",
        json!({}),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.message()["result"]["isError"], false);
    s.stop();
}

#[tokio::test]
async fn browser_origin_is_rejected_even_with_the_token() {
    let s = start(Grants::default()).await;
    let body = call("get_document_summary", json!({}));
    for origin in ["http://evil.example", "http://127.0.0.1", "null"] {
        let r = post(
            s.addr,
            &[V, ("Authorization", &bearer()), ("Origin", origin)],
            &body,
        )
        .await;
        assert_eq!(r.status, 403, "{origin}");
    }
    s.stop();
}

#[tokio::test]
async fn rebinding_host_is_rejected() {
    let s = start(Grants::default()).await;
    let body = call("get_document_summary", json!({}));
    let r = post(
        s.addr,
        &[
            V,
            ("Authorization", &bearer()),
            ("Host", "attacker.example"),
        ],
        &body,
    )
    .await;
    assert!(
        (400..500).contains(&r.status),
        "status {} for a foreign Host",
        r.status
    );
    s.stop();
}

#[tokio::test]
async fn oversize_body_is_rejected() {
    let s = start(Grants::headless(true, true, false)).await;
    let big = "x".repeat(locus_mcp::policy::MAX_BODY_BYTES + 1);
    let body = call("set_parameter", json!({"name": "p", "expr": big}));
    let r = post(s.addr, &[V, ("Authorization", &bearer())], &body).await;
    assert_eq!(r.status, 413);
    s.stop();
}

#[tokio::test]
async fn scopes_are_enforced_over_http() {
    let s = start(Grants::default()).await;
    let r = post_call(
        s.addr,
        &[("Authorization", &bearer())],
        "add_entities",
        json!({"entities": [{"type": "point", "x": 0.0, "y": 0.0}]}),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    let m = r.message();
    assert_eq!(m["result"]["isError"], true);
    assert_eq!(
        m["result"]["structuredContent"]["error"]["kind"],
        "scope_denied"
    );
    s.stop();
}

#[tokio::test]
async fn legacy_initialize_handshake_works() {
    let s = start(Grants::default()).await;
    let r = post(
        s.addr,
        &[("Authorization", &bearer())],
        &json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25", "capabilities": {},
                "clientInfo": {"name": "t", "version": "0"}
            }
        })
        .to_string(),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.message()["result"]["serverInfo"]["name"], "locus");
    s.stop();
}

#[tokio::test]
async fn other_paths_and_stop() {
    let s = start(Grants::default()).await;
    let mut c = TcpStream::connect(s.addr).await.unwrap();
    c.write_all(b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut out = String::new();
    let _ = c.read_to_string(&mut out).await;
    assert!(out.starts_with("HTTP/1.1 404"), "{out}");
    s.stop();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let r = TcpStream::connect(s.addr).await;
    assert!(r.is_err(), "the listener is closed after stop");
}
