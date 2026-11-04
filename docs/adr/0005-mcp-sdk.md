# ADR-0005: MCP SDK — `rmcp` 3.8, MCP revision 2026-07-28 (with 2025-11-25 fallback)

- **Status:** Accepted
- **Date:** 2026-11-04
- **Spec:** `docs/SPEC.md §16.2` (Implementation), closes §12 item 8
- **Issue:** Nerdtisan/Locus#49

## Context

`locus-mcp` is a thin adapter from MCP to the §11 command API, using the official Rust SDK `rmcp` (§16.2). Two transports: **stdio** (`locus-cli mcp --doc file.locus`, headless) and **Streamable HTTP on localhost** (attached to the running GUI). §16.4 is non-negotiable: off by default, 127.0.0.1 only, per-session bearer token, `Origin` validation against DNS rebinding, read/write/export scopes, approval modes, labelled undo transactions, rate limits. Desktop-only at first (§0), so `locus-mcp` is not one of the wasm32 core crates. Dependencies must pass `deny.toml` (§2).

This ADR pins the SDK version and the MCP spec revision, and records what to take from SolidCraft's MCP server (§16.2, §18).

## Options

### Option A — `rmcp` (official SDK, `modelcontextprotocol/rust-sdk`)

Maintained by the MCP project; server and client; macros for tool routing; JSON Schema for tool inputs via `schemars`; stdio and Streamable HTTP server transports.

### Option B — hand-rolled JSON-RPC (as SolidCraft does)

SolidCraft's `solidcraft-mcp` crate depends only on its engine and `serde_json` and implements JSON-RPC over stdio itself, advertising revisions 2025-06-18, 2025-03-26 and 2024-11-05. It works, but every spec revision becomes our maintenance job, and Streamable HTTP (needed for GUI attach) is considerably more protocol than stdio.

### MCP revision candidates

- **2026-07-28** — current specification, released 2026-07-28. Makes the protocol stateless: removes the `initialize` handshake (SEP-2575) and the `Mcp-Session-Id` protocol session (SEP-2567); lifecycle data moves into per-request `_meta`; standard HTTP headers (SEP-2243).
- **2025-11-25** — previous revision; the last one with the `initialize` handshake and protocol sessions. Many deployed clients will still speak it for a while.

## Measurements

Checked on 2026-11-04 with `cargo info` against crates.io and the `rust-sdk` GitHub releases, and with a probe crate (`rmcp = "=3.8.0"`, `default-features = false`, features `server`, `macros`, `transport-io`, `transport-streamable-http-server`, plus `tokio`) built in the `locus-dev` container and checked with this repo's `deny.toml`. Source read from the downloaded `rmcp-3.8.0` crate.

| Criterion | Finding |
|---|---|
| Latest release | `rmcp` **3.8.0** (2026-10-26); 3.7.0 four days earlier — releases are frequent |
| Licence | Apache-2.0 |
| `cargo deny check` on the probe's tree (85 crates) | **advisories ok, bans ok, licences ok, sources ok** |
| MSRV | 1.88 (workspace `rust-version` is 1.92 — fine) |
| Protocol versions known (`ProtocolVersion::KNOWN_VERSIONS`) | 2024-11-05, 2025-03-26, 2025-06-18, 2025-11-25, **2026-07-28** (`LATEST`) |
| Legacy handshake | `LATEST_WITH_INITIALIZE` = 2025-11-25; the server answers `initialize` for older clients and serves 2026-07-28 requests statelessly |
| stdio transport | feature `transport-io` |
| Streamable HTTP server | feature `transport-streamable-http-server`; a `tower` service (`StreamableHttpService`), so we own the listener and bind address |
| `Host` validation | `StreamableHttpServerConfig::allowed_hosts`, **defaults to loopback hosts only** |
| `Origin` validation | `allowed_origins`, **defaults to empty = validation off**; `enforce_origin_validation()` rejects every present `Origin` with an empty list. Requests *without* `Origin` still pass |
| Bearer-token auth (server side) | **No built-in hook.** The `auth` feature is client-side OAuth (`reqwest`, `oauth2`). Server auth is a `tower`/`axum` layer we put in front of the service |
| Tool annotations | `ToolAnnotations` (`read_only_hint`, `destructive_hint`, …) supported |
| Tool input schemas | `schemars` 1.x derives, pulled in by `server` |

Notes: the official 2026-07-28 announcement listed the Rust SDK's support as beta at launch; in 3.8.0 it is the default `LATEST`. Not measured: interoperability with specific clients (Claude Desktop, Claude Code, others) — that needs a running server, which is M3/M4 work.

## SolidCraft review (reference only, §18)

[`tangency-labs/solidcraft`](https://github.com/tangency-labs/solidcraft) (MIT OR Apache-2.0), `docs/mcp.md` and `crates/mcp/`, read on 2026-11-04. Worth copying as *design* (no code is taken):

- **The MCP surface is the command API.** Generic tools `list_commands`, `describe_command` (schemas), `execute(command, args)` and `batch` run the same engine commands as the toolbar and scripts, beside a few typed tools (`inspect_design`, `measure`, `set_parameter`, `undo`/`redo`, `screenshot`, `compare_picture`, `export`, `open`/`save`). This is exactly §11's intent.
- **Typed parameter contracts with a check level** (`off` / `warn` / `strict`), returning "did you mean …" hints — useful structured errors for AI self-correction (§16.3).
- **Tool annotations on every tool** (read-only / destructive / open-world), so clients that auto-approve on hints behave sensibly.
- **Headless vs bridged:** the CLI either runs its own in-process session or bridges to the running app over a loopback control channel with a **fresh token per app launch, stored in a file readable only by the user** (prefer an env var over a CLI flag, since arguments are visible in the process list).
- **Path allow-list** for any tool that touches files, with careful Windows path rules (UNC, device names, 8.3 names, alternate data streams refused).
- **Screenshots** so the agent can see its result (our `render_preview`).

Where Locus differs on purpose: SolidCraft's app control channel is on by default; §16.4 makes Locus's MCP **off by default**. SolidCraft lets agents save files inside allowed folders; §16.4 allows only host-mediated, user-confirmed import/export, which is stricter and stays.

## Decision

1. **SDK:** `rmcp` with `version = "3.8"` (caret within 3.x via `Cargo.lock`; minor/patch updates allowed per CLAUDE.md, a 4.x bump needs a new decision). Features: `server`, `macros`, `transport-io`, `transport-streamable-http-server`; `default-features = false`. Not `auth` (client-side only).
2. **Spec revision: target 2026-07-28**, the current revision, and **also accept 2025-11-25** clients through `rmcp`'s legacy `initialize` path. Drop the legacy path once the main clients have moved (revisit at M4). Older revisions (2025-06-18 and before) are not tested or promised.
3. **Streamable HTTP is hardened by `locus-mcp`, not by defaults:** bind a `TcpListener` to `127.0.0.1` ourselves (never `0.0.0.0`; reject any other address at config time); keep `allowed_hosts` at loopback; **always call `enforce_origin_validation()`** and reject any request carrying a browser `Origin` (a local AI client sends none); a `tower` layer checks `Authorization: Bearer <token>` with a constant-time compare before the request reaches `rmcp`; rate-limit and payload-cap layers sit in the same stack (§16.4).
4. **Tool design follows §16.3's typed tools**, borrowing from SolidCraft: tool annotations on every tool, schemas derived from the §11 command types, structured errors with entity/constraint IDs (ADR-0003), and a `batch` that is one labelled undo transaction (§16.4). A generic `execute_command` over the command API is a candidate for M3, but only if per-command scope metadata (read / write / export) makes scope enforcement as strict as per-tool scopes.

Chosen over hand-rolling because the SDK tracks spec revisions (four in the last two years, the latest a breaking one), already has a Streamable HTTP server, and passes our licence policy.

## Consequences

- `locus-mcp` gains `rmcp`, `tokio` and `schemars`; all desktop-only and outside the wasm32 core set.
- **Spec wording to flag, not silently resolve (§17):** §16.4 says "per-session bearer token" and the approval mode "ask per session". Revision 2026-07-28 has no protocol sessions. This ADR reads "session" as an **app-level MCP session**: from the moment the user enables MCP (or launches with it enabled) until they disable it or quit; the token is regenerated each time. If that reading is wrong, §16.4 needs a decision.
- Required tests (§16.5) gain two from this ADR: a request with a browser `Origin` is rejected, and a request with a non-loopback `Host` is rejected — both guard against `rmcp` default changes.
- Releases are frequent; review the `rmcp` changelog when bumping, especially for transport-security defaults.
- Revisit if `rmcp` stops tracking the spec, changes licence, or if a future revision changes the transport model again.

Sources: [crates.io `rmcp`](https://crates.io/crates/rmcp), [rust-sdk releases](https://github.com/modelcontextprotocol/rust-sdk/releases), [MCP blog: 2026-07-28 release](https://blog.modelcontextprotocol.io/posts/2026-07-28/), [SolidCraft `docs/mcp.md`](https://github.com/tangency-labs/solidcraft/blob/main/docs/mcp.md).
