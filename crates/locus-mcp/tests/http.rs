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
    start_with(grants, http::Access::default()).await
}

async fn start_with(grants: Grants, access: http::Access) -> http::Running {
    let backend = LocalBackend::spawn(Headless::new(locus_doc::Document::new())).unwrap();
    let server = LocusServer::new(backend, Arc::new(NeverApprove), grants, Options::default());
    http::start("127.0.0.1:0".parse().unwrap(), server, TOKEN.into(), access)
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
            let r = http::start(
                addr.parse().unwrap(),
                server,
                TOKEN.into(),
                http::Access {
                    allow_docker: true,
                    ..Default::default()
                },
            )
            .await;
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

// ── #270: opt-in Docker clients (host.docker.internal) ─────────────────────

async fn summary_from(s: &http::Running, host: &str, origin: Option<&str>) -> Reply {
    let auth = bearer();
    let mut h = vec![("Authorization", auth.as_str()), ("Host", host)];
    if let Some(o) = origin {
        h.push(("Origin", o));
    }
    post_call(s.addr, &h, "get_document_summary", json!({})).await
}

#[tokio::test]
async fn docker_host_is_refused_by_default() {
    let s = start(Grants::default()).await;
    let host = format!("host.docker.internal:{}", s.addr.port());
    let r = summary_from(&s, &host, None).await;
    assert_eq!(r.status, 403, "{}", r.body);
    let r = summary_from(&s, "host.docker.internal", None).await;
    assert_eq!(r.status, 403);
    s.stop();
}

#[tokio::test]
async fn docker_host_is_accepted_when_allowed() {
    let s = start_with(
        Grants::default(),
        http::Access {
            allow_docker: true,
            ..Default::default()
        },
    )
    .await;
    let host = format!("host.docker.internal:{}", s.addr.port());
    let r = summary_from(&s, &host, None).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.message()["result"]["isError"], false);
    let r = summary_from(&s, "HOST.DOCKER.INTERNAL", None).await;
    assert_eq!(r.status, 200, "without a port, any case: {}", r.body);
    // Loopback names still work.
    let r = summary_from(&s, &format!("127.0.0.1:{}", s.addr.port()), None).await;
    assert_eq!(r.status, 200);
    s.stop();
}

#[tokio::test]
async fn allowing_docker_lets_in_nothing_else() {
    let s = start_with(
        Grants::default(),
        http::Access {
            allow_docker: true,
            ..Default::default()
        },
    )
    .await;
    for host in [
        "attacker.example",
        "host.docker.internal.attacker.example",
        "docker.internal",
        "gateway.docker.internal",
        "192.168.65.254",
    ] {
        let r = summary_from(&s, host, None).await;
        assert!((400..500).contains(&r.status), "{host}: {}", r.status);
    }
    // A browser Origin is refused whatever the Host.
    let host = format!("host.docker.internal:{}", s.addr.port());
    for origin in ["http://evil.example", "http://host.docker.internal:3000"] {
        assert_eq!(
            summary_from(&s, &host, Some(origin)).await.status,
            403,
            "{origin}"
        );
    }
    // And the token is still required.
    let r = post_call(
        s.addr,
        &[("Host", host.as_str())],
        "get_document_summary",
        json!({}),
    )
    .await;
    assert_eq!(r.status, 401);
    s.stop();
}

#[tokio::test]
async fn origin_is_refused_without_docker_too() {
    let s = start(Grants::default()).await;
    let host = format!("127.0.0.1:{}", s.addr.port());
    assert_eq!(
        summary_from(&s, &host, Some("http://localhost:3000"))
            .await
            .status,
        403
    );
    s.stop();
}

// ── #272: opt-in network clients (private bind + allowlist) ────────────────

/// This machine's first private address, or `None` (the LAN tests then skip).
fn private_ip() -> Option<std::net::Ipv4Addr> {
    let ip = http::private_addresses().into_iter().next();
    if ip.is_none() {
        eprintln!("no private IPv4 interface here: skipping a LAN test");
    }
    ip
}

fn lan_access(bind: std::net::Ipv4Addr, allow: &[String]) -> http::Access {
    http::Access {
        allow_docker: false,
        network: Some(http::Network {
            bind,
            allow: allow.iter().map(|s| s.parse().unwrap()).collect(),
        }),
    }
}

#[tokio::test]
async fn network_off_listens_on_loopback_only() {
    let Some(ip) = private_ip() else { return };
    let s = start(Grants::default()).await;
    assert!(s.network_addr.is_none());
    let r = TcpStream::connect((ip, s.addr.port())).await;
    assert!(r.is_err(), "nothing listens on the LAN address while off");
    s.stop();
}

#[tokio::test]
async fn empty_allowlist_admits_nobody_from_the_network() {
    let Some(ip) = private_ip() else { return };
    let s = start_with(Grants::default(), lan_access(ip, &[])).await;
    let lan = s.network_addr.unwrap();
    assert_eq!(lan.ip(), std::net::IpAddr::V4(ip));
    let auth = bearer();
    let r = post_call(
        lan,
        &[("Authorization", auth.as_str())],
        "get_document_summary",
        json!({}),
    )
    .await;
    assert_eq!(r.status, 0, "connection dropped before HTTP: {}", r.body);
    // Loopback still works.
    let r = post_call(
        s.addr,
        &[("Authorization", auth.as_str())],
        "get_document_summary",
        json!({}),
    )
    .await;
    assert_eq!(r.status, 200);
    s.stop();
}

#[tokio::test]
async fn client_outside_the_allowlist_is_dropped() {
    let Some(ip) = private_ip() else { return };
    // A range that can't contain this machine's address.
    let other = if ip.octets()[0] == 10 {
        "192.168.0.0/16"
    } else {
        "10.0.0.0/8"
    };
    let s = start_with(Grants::default(), lan_access(ip, &[other.to_owned()])).await;
    let auth = bearer();
    let r = post_call(
        s.network_addr.unwrap(),
        &[("Authorization", auth.as_str())],
        "get_document_summary",
        json!({}),
    )
    .await;
    assert_eq!(r.status, 0);
    s.stop();
}

#[tokio::test]
async fn allowlisted_client_still_needs_token_and_no_origin() {
    let Some(ip) = private_ip() else { return };
    let s = start_with(Grants::default(), lan_access(ip, &[ip.to_string()])).await;
    let lan = s.network_addr.unwrap();
    // No token.
    let r = post_call(lan, &[], "get_document_summary", json!({})).await;
    assert_eq!(r.status, 401);
    // Origin, even with the token.
    let auth = bearer();
    let r = post_call(
        lan,
        &[
            ("Authorization", auth.as_str()),
            ("Origin", "http://evil.example"),
        ],
        "get_document_summary",
        json!({}),
    )
    .await;
    assert_eq!(r.status, 403);
    // A foreign Host.
    let r = post_call(
        lan,
        &[
            ("Authorization", auth.as_str()),
            ("Host", "attacker.example"),
        ],
        "get_document_summary",
        json!({}),
    )
    .await;
    assert_eq!(r.status, 403);
    // Token, the LAN address as Host: in. Scopes still apply.
    let r = post_call(
        lan,
        &[("Authorization", auth.as_str())],
        "get_document_summary",
        json!({}),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post_call(
        lan,
        &[("Authorization", auth.as_str())],
        "add_entities",
        json!({"entities": [{"type": "point", "x": 0.0, "y": 0.0}]}),
    )
    .await;
    assert_eq!(
        r.message()["result"]["structuredContent"]["error"]["kind"],
        "scope_denied"
    );
    s.stop();
}

#[test]
fn public_or_unspecified_network_bind_is_refused() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        for bind in [
            std::net::Ipv4Addr::UNSPECIFIED,
            std::net::Ipv4Addr::new(8, 8, 8, 8),
        ] {
            let backend = LocalBackend::spawn(Headless::new(locus_doc::Document::new())).unwrap();
            let server = LocusServer::new(
                backend,
                Arc::new(NeverApprove),
                Grants::default(),
                Options::default(),
            );
            let access = http::Access {
                allow_docker: false,
                network: Some(http::Network {
                    bind,
                    allow: vec![],
                }),
            };
            let r = http::start("127.0.0.1:0".parse().unwrap(), server, TOKEN.into(), access).await;
            assert!(matches!(r, Err(HttpError::NotLoopback(_))), "{bind}");
        }
    });
}
