// SPDX-License-Identifier: AGPL-3.0-or-later

//! MCP server exposing document operations to the user's own AI client, as a
//! thin adapter over the command API (SPEC §16, ADR 0005). Desktop-only (SPEC
//! §0): the app and CLI depend on it only off wasm32.
//!
//! - [`tools`]: the §16.3 tools as plain functions over a [`host::Workspace`];
//!   every write is one labelled `Command::Compound`, so one undo step.
//! - [`policy`]: scopes, approval modes, rate limits and caps (§16.4).
//! - [`server`]: the `rmcp` handler that puts every call through that policy.
//!
//! Never logs arguments, document contents, file paths or tokens (§16.4).

// Tests may unwrap; library code may not (CLAUDE.md, Code Style).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod attach;
pub mod dto;
pub mod error;
pub mod host;
pub mod http;
pub mod plugins;
pub mod policy;
pub mod preview;
pub mod server;
pub mod stdio;
pub mod tools;

pub use error::ToolError;
pub use host::{
    ApprovalRequest, Approver, Backend, Headless, LocalBackend, NeverApprove, PluginEntry, Plugins,
    Workspace,
};
pub use policy::{ApprovalMode, Grants, Scope};
pub use server::{LocusServer, Options};
