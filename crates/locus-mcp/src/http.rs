// SPDX-License-Identifier: AGPL-3.0-or-later

//! Streamable HTTP on localhost, attached to the running app (SPEC §16.2,
//! §16.4, ADR 0005 decision 3; #229).
//!
//! Hardened here, not by library defaults. Every request passes, in order:
//! 1. **Origin**: any `Origin` header is refused. Browsers always send one;
//!    a local AI client does not. This blocks DNS rebinding and drive-by
//!    pages (rmcp's own Origin check is also enforced, as a second line).
//! 2. **Host**: loopback names only (rmcp's `allowed_hosts`).
//! 3. **Bearer token**: `Authorization: Bearer <token>`, compared in
//!    constant time. The token is per MCP session (ADR 0005: from enabling
//!    MCP until it is disabled or the app quits).
//! 4. **Size**: bodies over [`MAX_BODY_BYTES`] are refused.
//!
//! The listener binds `127.0.0.1` and nothing else; [`check_bind`] refuses
//! any other address before a socket is opened.

use std::convert::Infallible;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use base64::Engine as _;
use bytes::Bytes;
use http::{Request, Response, StatusCode, header};
use http_body_util::{BodyExt, Full, combinators::BoxBody};
use hyper::body::Incoming;
use hyper_util::rt::TokioIo;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::policy::MAX_BODY_BYTES;
use crate::server::LocusServer;

/// The one path the server answers on.
pub const PATH: &str = "/mcp";

/// Why the HTTP transport could not start.
#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    /// SPEC §16.4: 127.0.0.1 only, never 0.0.0.0 or any other interface.
    #[error("refusing to listen on {0}: the MCP server binds 127.0.0.1 only")]
    NotLoopback(IpAddr),
    #[error("cannot listen: {0}")]
    Io(#[from] std::io::Error),
    #[error("cannot create a session token: {0}")]
    Random(String),
}

/// Accept only 127.0.0.1 (IPv4 loopback; `::1` is refused too so there is
/// exactly one address to reason about).
///
/// # Errors
/// [`HttpError::NotLoopback`] for any other address.
pub fn check_bind(addr: SocketAddr) -> Result<SocketAddr, HttpError> {
    if addr.ip() == IpAddr::V4(Ipv4Addr::LOCALHOST) {
        Ok(addr)
    } else {
        Err(HttpError::NotLoopback(addr.ip()))
    }
}

/// A fresh random bearer token (256 bits, URL-safe base64).
///
/// # Errors
/// The OS random source failed.
pub fn new_token() -> Result<String, HttpError> {
    let mut b = [0u8; 32];
    getrandom::fill(&mut b).map_err(|e| HttpError::Random(e.to_string()))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b))
}

/// Equal without leaking where the first difference is.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

type Body = BoxBody<Bytes, Infallible>;

fn reply(status: StatusCode, msg: &'static str) -> Response<Body> {
    let mut r = Response::new(Full::new(Bytes::from_static(msg.as_bytes())).boxed());
    *r.status_mut() = status;
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    if status == StatusCode::UNAUTHORIZED {
        r.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            header::HeaderValue::from_static("Bearer"),
        );
    }
    r
}

/// The checks before rmcp sees a request; `Some` is the refusal.
fn refuse<B>(req: &Request<B>, token: &str) -> Option<Response<Body>> {
    if req.uri().path() != PATH {
        return Some(reply(StatusCode::NOT_FOUND, "not found"));
    }
    if req.headers().contains_key(header::ORIGIN) {
        return Some(reply(
            StatusCode::FORBIDDEN,
            "browser requests are not accepted",
        ));
    }
    let given = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim);
    match given {
        Some(t) if same(t.as_bytes(), token.as_bytes()) => None,
        _ => Some(reply(
            StatusCode::UNAUTHORIZED,
            "missing or invalid bearer token",
        )),
    }
}

/// A running HTTP server. Dropping it does not stop it; call [`Self::stop`].
#[derive(Debug)]
pub struct Running {
    pub addr: SocketAddr,
    cancel: CancellationToken,
}

impl Running {
    /// Stop accepting connections and end every MCP stream.
    pub fn stop(&self) {
        self.cancel.cancel();
    }
}

/// Listen on `addr` (must be 127.0.0.1; port 0 picks a free one) and serve
/// `server` to clients presenting `token`. Runs on the current tokio runtime.
///
/// # Errors
/// A non-loopback address or the port is taken.
pub async fn start(
    addr: SocketAddr,
    server: LocusServer,
    token: String,
) -> Result<Running, HttpError> {
    let addr = check_bind(addr)?;
    let listener = TcpListener::bind(addr).await?;
    let addr = listener.local_addr()?;
    let cancel = CancellationToken::new();
    let config = StreamableHttpServerConfig::default()
        .with_allowed_hosts(["127.0.0.1", "localhost"])
        .enforce_origin_validation()
        .with_max_request_body_bytes(MAX_BODY_BYTES)
        .with_cancellation_token(cancel.child_token());
    let service = StreamableHttpService::new(
        move || Ok(server.clone()),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    let token: Arc<str> = token.into();
    let stop = cancel.clone();
    tokio::spawn(async move {
        loop {
            let (stream, _) = tokio::select! {
                () = stop.cancelled() => break,
                r = listener.accept() => match r {
                    Ok(c) => c,
                    Err(_) => continue,
                },
            };
            let service = service.clone();
            let token = token.clone();
            let stop = stop.clone();
            tokio::spawn(async move {
                let svc = hyper::service::service_fn(move |req: Request<Incoming>| {
                    let mut service = service.clone();
                    let token = token.clone();
                    async move {
                        if let Some(r) = refuse(&req, &token) {
                            return Ok::<_, Infallible>(r);
                        }
                        let r = tower_service::Service::call(&mut service, req).await?;
                        Ok(r.map(|b| b.boxed()))
                    }
                });
                let conn = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), svc);
                tokio::select! {
                    _ = conn => {}
                    () = stop.cancelled() => {}
                }
            });
        }
    });
    Ok(Running { addr, cancel })
}

/// The HTTP server on its own runtime, for a GUI that has none. Stops when
/// dropped (the user turned MCP off or quit).
#[derive(Debug)]
pub struct Attached {
    rt: Option<tokio::runtime::Runtime>,
    running: Running,
}

impl Attached {
    /// Listen on `127.0.0.1:port` (0 picks a free port).
    ///
    /// # Errors
    /// The runtime could not start or the port is taken.
    pub fn start(port: u16, server: LocusServer, token: String) -> Result<Self, HttpError> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("locus-mcp-http")
            .enable_all()
            .build()?;
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        let running = rt.block_on(start(addr, server, token))?;
        Ok(Self {
            rt: Some(rt),
            running,
        })
    }

    #[must_use]
    pub fn addr(&self) -> SocketAddr {
        self.running.addr
    }

    /// The URL a client connects to.
    #[must_use]
    pub fn url(&self) -> String {
        format!("http://{}{PATH}", self.running.addr)
    }
}

impl Drop for Attached {
    fn drop(&mut self) {
        self.running.stop();
        if let Some(rt) = self.rt.take() {
            // Never block the UI thread on a stuck connection.
            rt.shutdown_background();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_ipv4_loopback_binds() {
        assert!(check_bind("127.0.0.1:0".parse().unwrap()).is_ok());
        for bad in [
            "0.0.0.0:0",
            "192.168.1.2:8080",
            "[::1]:0",
            "[::]:0",
            "127.0.0.2:0",
        ] {
            assert!(
                matches!(
                    check_bind(bad.parse().unwrap()),
                    Err(HttpError::NotLoopback(_))
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn tokens_are_random_and_long() {
        let a = new_token().unwrap();
        let b = new_token().unwrap();
        assert_ne!(a, b);
        assert_eq!(a.len(), 43);
    }

    #[test]
    fn constant_time_compare() {
        assert!(same(b"abc", b"abc"));
        assert!(!same(b"abc", b"abd"));
        assert!(!same(b"abc", b"abcd"));
    }
}
