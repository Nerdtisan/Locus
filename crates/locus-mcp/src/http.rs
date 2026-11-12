// SPDX-License-Identifier: AGPL-3.0-or-later

//! Streamable HTTP, attached to the running app (SPEC §16.2, §16.4, ADR 0005
//! decision 3; #229, #270, #272).
//!
//! Hardened here, not by library defaults. Every connection and request
//! passes, in order:
//! 1. **Peer address** (at accept, before any HTTP): loopback always; with
//!    [`Access::network`] on, also clients in its allowlist. Everyone else
//!    is disconnected unread.
//! 2. **Origin**: any `Origin` header is refused. Browsers always send one;
//!    a local AI client does not. This blocks DNS rebinding and drive-by
//!    pages (rmcp's own Origin check is also enforced, as a second line).
//! 3. **Host**: `127.0.0.1` or `localhost`; plus `host.docker.internal`
//!    with [`Access::allow_docker`] (#270) and the bound LAN address with
//!    [`Access::network`] (#272). Checked here and again by rmcp's
//!    `allowed_hosts`.
//! 4. **Bearer token**: `Authorization: Bearer <token>`, compared in
//!    constant time.
//! 5. **Size**: bodies over [`MAX_BODY_BYTES`] are refused.
//!
//! The server binds `127.0.0.1`. Only with [`Access::network`] does it also
//! bind one private (RFC 1918 or link-local) IPv4 address the user chose;
//! [`check_bind`] refuses `0.0.0.0` and public addresses before a socket
//! is opened. Plain HTTP on a LAN can be sniffed: the app says so and
//! recommends an SSH tunnel or a VPN to the loopback server instead.

use std::convert::Infallible;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

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

/// Docker Desktop's name for the host, as seen from a container. Docker
/// forwards it to the host's loopback, so the bind stays 127.0.0.1.
pub const DOCKER_HOST: &str = "host.docker.internal";

/// Why the HTTP transport could not start.
#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    /// SPEC §16.4: 127.0.0.1, or the one private address the user chose
    /// for network clients; never 0.0.0.0, public or any other address.
    #[error(
        "refusing to listen on {0}: the MCP server binds 127.0.0.1, or a chosen private address for network clients"
    )]
    NotLoopback(IpAddr),
    #[error("cannot listen: {0}")]
    Io(#[from] std::io::Error),
    #[error("cannot create a session token: {0}")]
    Random(String),
}

/// RFC 1918 private or link-local IPv4: the only addresses a LAN bind or
/// allowlist entry may use (#272).
#[must_use]
pub fn is_private(ip: Ipv4Addr) -> bool {
    ip.is_private() || ip.is_link_local()
}

/// One allowlist entry: an IPv4 address or CIDR range wholly inside a
/// private block (10/8, 172.16/12, 192.168/16, 169.254/16).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cidr {
    net: Ipv4Addr,
    prefix: u8,
}

impl Cidr {
    fn mask(prefix: u8) -> u32 {
        if prefix == 0 {
            0
        } else {
            u32::MAX << (32 - u32::from(prefix))
        }
    }

    #[must_use]
    pub fn contains(&self, ip: Ipv4Addr) -> bool {
        let m = Self::mask(self.prefix);
        u32::from(ip) & m == u32::from(self.net) & m
    }
}

impl std::fmt::Display for Cidr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.prefix == 32 {
            write!(f, "{}", self.net)
        } else {
            write!(f, "{}/{}", self.net, self.prefix)
        }
    }
}

impl FromStr for Cidr {
    type Err = String;

    /// `192.168.1.20` or `192.168.1.0/24`. Refuses anything reaching outside
    /// the private blocks (e.g. `10.0.0.0/7`, `0.0.0.0/0`, public addresses).
    fn from_str(s: &str) -> Result<Self, String> {
        let s = s.trim();
        let (ip, prefix) = match s.split_once('/') {
            Some((ip, p)) => (
                ip,
                p.parse::<u8>()
                    .ok()
                    .filter(|p| *p <= 32)
                    .ok_or_else(|| format!("`{s}`: the prefix must be 0–32"))?,
            ),
            None => (s, 32),
        };
        let ip: Ipv4Addr = ip
            .parse()
            .map_err(|_| format!("`{s}` is not an IPv4 address or range"))?;
        let net = Ipv4Addr::from(u32::from(ip) & Self::mask(prefix));
        // The whole range must sit inside one private block.
        let blocks: [(Ipv4Addr, u8); 4] = [
            (Ipv4Addr::new(10, 0, 0, 0), 8),
            (Ipv4Addr::new(172, 16, 0, 0), 12),
            (Ipv4Addr::new(192, 168, 0, 0), 16),
            (Ipv4Addr::new(169, 254, 0, 0), 16),
        ];
        let inside = blocks.iter().any(|(b, bp)| {
            prefix >= *bp
                && Cidr {
                    net: *b,
                    prefix: *bp,
                }
                .contains(net)
        });
        if !inside {
            return Err(format!(
                "`{s}` is not inside a private network (10.x, 172.16–31.x, 192.168.x, 169.254.x)"
            ));
        }
        Ok(Cidr { net, prefix })
    }
}

/// Network clients (#272): one private address to bind, and who may connect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Network {
    pub bind: Ipv4Addr,
    /// Empty by default: nobody gets in until the user adds a client.
    pub allow: Vec<Cidr>,
}

/// Who may reach the server besides loopback clients (SPEC §16.4). The
/// token, Origin refusal, scopes, approvals and caps apply regardless.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Access {
    /// Accept `Host: host.docker.internal[:port]`, so MCP clients in Docker
    /// Desktop containers on this PC (e.g. Open WebUI) can connect. Off by
    /// default (owner decision 2026-11-12, #270).
    pub allow_docker: bool,
    /// Also listen on a private LAN address for allowlisted clients. Off by
    /// default (owner decision 2026-11-12, #272).
    pub network: Option<Network>,
}

impl Access {
    /// The host names accepted in the `Host` header.
    #[must_use]
    pub fn hosts(&self) -> Vec<String> {
        let mut h = vec!["127.0.0.1".to_owned(), "localhost".to_owned()];
        if self.allow_docker {
            h.push(DOCKER_HOST.to_owned());
        }
        if let Some(n) = &self.network {
            h.push(n.bind.to_string());
        }
        h
    }

    /// Whether a connection from `peer` may proceed to HTTP at all.
    #[must_use]
    pub fn admit(&self, peer: IpAddr) -> bool {
        if peer.is_loopback() {
            return true;
        }
        let v4 = match peer {
            IpAddr::V4(v) => v,
            IpAddr::V6(v) => match v.to_ipv4_mapped() {
                Some(v) => v,
                None => return false,
            },
        };
        self.network
            .as_ref()
            .is_some_and(|n| n.allow.iter().any(|c| c.contains(v4)))
    }
}

/// Accept 127.0.0.1, or the private address chosen for network clients
/// (`::1` is refused too, so there is exactly one loopback to reason about).
///
/// # Errors
/// [`HttpError::NotLoopback`] for any other address, including `0.0.0.0`
/// and public addresses.
pub fn check_bind(addr: SocketAddr, access: &Access) -> Result<SocketAddr, HttpError> {
    let ok = match addr.ip() {
        IpAddr::V4(ip) if ip == Ipv4Addr::LOCALHOST => true,
        IpAddr::V4(ip) => access
            .network
            .as_ref()
            .is_some_and(|n| n.bind == ip && is_private(ip)),
        IpAddr::V6(_) => false,
    };
    if ok {
        Ok(addr)
    } else {
        Err(HttpError::NotLoopback(addr.ip()))
    }
}

/// This PC's private IPv4 addresses, for choosing a LAN bind (#272).
#[must_use]
pub fn private_addresses() -> Vec<Ipv4Addr> {
    let mut v: Vec<Ipv4Addr> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|i| match i.ip() {
            IpAddr::V4(ip) if is_private(ip) => Some(ip),
            _ => None,
        })
        .collect();
    v.sort_unstable();
    v.dedup();
    v
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

/// The host name of a `Host` header value, without the port, lower-cased.
fn host_name(value: &str) -> String {
    let v = value.trim();
    let name = match v.rsplit_once(':') {
        Some((n, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => n,
        _ => v,
    };
    name.to_ascii_lowercase()
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
fn refuse<B>(req: &Request<B>, token: &str, hosts: &[String]) -> Option<Response<Body>> {
    if req.uri().path() != PATH {
        return Some(reply(StatusCode::NOT_FOUND, "not found"));
    }
    if req.headers().contains_key(header::ORIGIN) {
        return Some(reply(
            StatusCode::FORBIDDEN,
            "browser requests are not accepted",
        ));
    }
    let host_ok = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(host_name)
        .is_some_and(|h| hosts.contains(&h));
    if !host_ok {
        return Some(reply(StatusCode::FORBIDDEN, "host not allowed"));
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
    /// The loopback address.
    pub addr: SocketAddr,
    /// The LAN address, when network clients are on.
    pub network_addr: Option<SocketAddr>,
    cancel: CancellationToken,
    clients: Arc<AtomicUsize>,
}

impl Running {
    /// Stop accepting connections and end every MCP stream.
    pub fn stop(&self) {
        self.cancel.cancel();
    }

    /// Open connections from other machines right now.
    #[must_use]
    pub fn network_clients(&self) -> usize {
        self.clients.load(Ordering::Relaxed)
    }
}

/// Counts a non-loopback connection while it is open.
struct Counted(Arc<AtomicUsize>);

impl Drop for Counted {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

type Service = StreamableHttpService<LocusServer, LocalSessionManager>;

struct Shared {
    service: Service,
    token: Arc<str>,
    hosts: Arc<[String]>,
    access: Access,
    stop: CancellationToken,
    clients: Arc<AtomicUsize>,
}

fn accept_loop(listener: TcpListener, shared: Arc<Shared>) {
    tokio::spawn(async move {
        loop {
            let (stream, peer) = tokio::select! {
                () = shared.stop.cancelled() => break,
                r = listener.accept() => match r {
                    Ok(c) => c,
                    Err(_) => continue,
                },
            };
            // Before any HTTP: who is calling (#272). Dropped unread.
            if !shared.access.admit(peer.ip()) {
                drop(stream);
                continue;
            }
            let counted = (!peer.ip().is_loopback()).then(|| {
                shared.clients.fetch_add(1, Ordering::Relaxed);
                Counted(shared.clients.clone())
            });
            let shared = shared.clone();
            tokio::spawn(async move {
                let _counted = counted;
                let s = shared.clone();
                let svc = hyper::service::service_fn(move |req: Request<Incoming>| {
                    let s = s.clone();
                    async move {
                        if let Some(r) = refuse(&req, &s.token, &s.hosts) {
                            return Ok::<_, Infallible>(r);
                        }
                        let mut service = s.service.clone();
                        let r = tower_service::Service::call(&mut service, req).await?;
                        Ok(r.map(|b| b.boxed()))
                    }
                });
                let conn = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), svc);
                tokio::select! {
                    _ = conn => {}
                    () = shared.stop.cancelled() => {}
                }
            });
        }
    });
}

/// Listen on `addr` (must be 127.0.0.1; port 0 picks a free one) and serve
/// `server` to clients presenting `token`, from the hosts `access` allows.
/// With [`Access::network`], also listen on that private address, same port.
/// Runs on the current tokio runtime.
///
/// # Errors
/// A refused address or the port is taken.
pub async fn start(
    addr: SocketAddr,
    server: LocusServer,
    token: String,
    access: Access,
) -> Result<Running, HttpError> {
    if addr.ip() != IpAddr::V4(Ipv4Addr::LOCALHOST) {
        return Err(HttpError::NotLoopback(addr.ip()));
    }
    let addr = check_bind(addr, &access)?;
    let network_bind = match &access.network {
        Some(n) => Some(check_bind(
            SocketAddr::from((n.bind, addr.port())),
            &access,
        )?),
        None => None,
    };
    let listener = TcpListener::bind(addr).await?;
    let addr = listener.local_addr()?;
    let lan = match network_bind {
        Some(b) => Some(TcpListener::bind(SocketAddr::from((b.ip(), addr.port()))).await?),
        None => None,
    };
    let network_addr = match &lan {
        Some(l) => Some(l.local_addr()?),
        None => None,
    };
    let cancel = CancellationToken::new();
    let hosts = access.hosts();
    let config = StreamableHttpServerConfig::default()
        .with_allowed_hosts(hosts.clone())
        .enforce_origin_validation()
        .with_max_request_body_bytes(MAX_BODY_BYTES)
        .with_cancellation_token(cancel.child_token());
    let service = StreamableHttpService::new(
        move || Ok(server.clone()),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    let clients = Arc::new(AtomicUsize::new(0));
    let shared = Arc::new(Shared {
        service,
        token: token.into(),
        hosts: hosts.into(),
        access,
        stop: cancel.clone(),
        clients: clients.clone(),
    });
    accept_loop(listener, shared.clone());
    if let Some(l) = lan {
        accept_loop(l, shared);
    }
    Ok(Running {
        addr,
        network_addr,
        cancel,
        clients,
    })
}

/// The HTTP server on its own runtime, for a GUI that has none. Stops when
/// dropped (the user turned MCP off or quit).
#[derive(Debug)]
pub struct Attached {
    rt: Option<tokio::runtime::Runtime>,
    running: Running,
}

impl Attached {
    /// Listen on `127.0.0.1:port` (0 picks a free port), plus the network
    /// address in `access` when set.
    ///
    /// # Errors
    /// The runtime could not start, an address was refused, or the port is taken.
    pub fn start(
        port: u16,
        server: LocusServer,
        token: String,
        access: Access,
    ) -> Result<Self, HttpError> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("locus-mcp-http")
            .enable_all()
            .build()?;
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        let running = rt.block_on(start(addr, server, token, access))?;
        Ok(Self {
            rt: Some(rt),
            running,
        })
    }

    #[must_use]
    pub fn addr(&self) -> SocketAddr {
        self.running.addr
    }

    /// The URL a client on this PC connects to.
    #[must_use]
    pub fn url(&self) -> String {
        format!("http://{}{PATH}", self.running.addr)
    }

    /// The URL for other machines, when network clients are on.
    #[must_use]
    pub fn network_url(&self) -> Option<String> {
        self.running
            .network_addr
            .map(|a| format!("http://{a}{PATH}"))
    }

    /// Open connections from other machines right now.
    #[must_use]
    pub fn network_clients(&self) -> usize {
        self.running.network_clients()
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

    fn lan(allow: &[&str]) -> Access {
        Access {
            allow_docker: false,
            network: Some(Network {
                bind: Ipv4Addr::new(192, 168, 1, 10),
                allow: allow.iter().map(|s| s.parse().unwrap()).collect(),
            }),
        }
    }

    #[test]
    fn only_loopback_binds_without_network() {
        let off = Access::default();
        assert!(check_bind("127.0.0.1:0".parse().unwrap(), &off).is_ok());
        for bad in [
            "0.0.0.0:0",
            "192.168.1.10:8080",
            "[::1]:0",
            "[::]:0",
            "127.0.0.2:0",
        ] {
            assert!(
                matches!(
                    check_bind(bad.parse().unwrap(), &off),
                    Err(HttpError::NotLoopback(_))
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn network_binds_only_the_chosen_private_address() {
        let a = lan(&[]);
        assert!(check_bind("192.168.1.10:1".parse().unwrap(), &a).is_ok());
        for bad in ["0.0.0.0:1", "192.168.1.11:1", "8.8.8.8:1", "[::]:1"] {
            assert!(check_bind(bad.parse().unwrap(), &a).is_err(), "{bad}");
        }
        // A public address is refused even if it was chosen.
        let public = Access {
            allow_docker: false,
            network: Some(Network {
                bind: Ipv4Addr::new(8, 8, 8, 8),
                allow: vec![],
            }),
        };
        assert!(check_bind("8.8.8.8:1".parse().unwrap(), &public).is_err());
        let any = Access {
            allow_docker: false,
            network: Some(Network {
                bind: Ipv4Addr::UNSPECIFIED,
                allow: vec![],
            }),
        };
        assert!(check_bind("0.0.0.0:1".parse().unwrap(), &any).is_err());
    }

    #[test]
    fn cidr_parsing_stays_private() {
        let c: Cidr = "192.168.1.0/24".parse().unwrap();
        assert!(c.contains(Ipv4Addr::new(192, 168, 1, 77)));
        assert!(!c.contains(Ipv4Addr::new(192, 168, 2, 1)));
        assert_eq!(c.to_string(), "192.168.1.0/24");
        let one: Cidr = "10.0.0.5".parse().unwrap();
        assert!(one.contains(Ipv4Addr::new(10, 0, 0, 5)));
        assert!(!one.contains(Ipv4Addr::new(10, 0, 0, 6)));
        for bad in [
            "8.8.8.8",
            "0.0.0.0/0",
            "10.0.0.0/7",
            "172.0.0.0/8",
            "192.168.0.0/15",
            "100.64.0.1",
            "192.168.1.0/33",
            "fe80::1",
            "lan",
        ] {
            assert!(bad.parse::<Cidr>().is_err(), "{bad}");
        }
    }

    #[test]
    fn admission() {
        let lo: IpAddr = "127.0.0.1".parse().unwrap();
        let peer: IpAddr = "192.168.1.50".parse().unwrap();
        assert!(Access::default().admit(lo));
        assert!(!Access::default().admit(peer), "off: loopback only");
        assert!(!lan(&[]).admit(peer), "empty allowlist admits nobody");
        assert!(lan(&["192.168.1.50"]).admit(peer));
        assert!(lan(&["192.168.1.0/24"]).admit(peer));
        assert!(!lan(&["192.168.2.0/24"]).admit(peer));
        assert!(!lan(&["192.168.1.0/24"]).admit("10.0.0.1".parse().unwrap()));
        assert!(
            lan(&["192.168.1.50"]).admit("::ffff:192.168.1.50".parse().unwrap()),
            "IPv4-mapped IPv6"
        );
        assert!(!lan(&["192.168.1.0/24"]).admit("fe80::1".parse().unwrap()));
    }

    #[test]
    fn tokens_are_random_and_long() {
        let a = new_token().unwrap();
        let b = new_token().unwrap();
        assert_ne!(a, b);
        assert_eq!(a.len(), 43);
    }

    #[test]
    fn host_names_and_ports() {
        assert_eq!(host_name("127.0.0.1:47811"), "127.0.0.1");
        assert_eq!(host_name("LOCALHOST"), "localhost");
        assert_eq!(
            host_name("host.docker.internal:47811"),
            "host.docker.internal"
        );
        assert_eq!(host_name("evil.example:"), "evil.example:");
        assert!(!Access::default().hosts().contains(&DOCKER_HOST.to_owned()));
        let d = Access {
            allow_docker: true,
            network: None,
        };
        assert!(d.hosts().contains(&DOCKER_HOST.to_owned()));
        assert!(lan(&[]).hosts().contains(&"192.168.1.10".to_owned()));
    }

    #[test]
    fn constant_time_compare() {
        assert!(same(b"abc", b"abc"));
        assert!(!same(b"abc", b"abd"));
        assert!(!same(b"abc", b"abcd"));
    }
}
