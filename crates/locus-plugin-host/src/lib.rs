// SPDX-License-Identifier: AGPL-3.0-or-later

//! wasmtime-based plugin runtime, capability enforcement and plugin registry
//! (SPEC §15, ADR 0009, ADR 0013). Desktop-only at first (SPEC §0): on `wasm32`
//! only the plain types, validation and manifest parsing build, so the web
//! build never links wasmtime.

// Tests may unwrap; library code may not (CLAUDE.md, Code Style).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod core_host;
pub mod doc;
pub mod error;
pub mod manifest;
pub mod types;
pub mod validate;

/// The plugin API this host implements (ADR 0009, §15.3).
pub mod api {
    /// The WIT package.
    pub const PACKAGE: &str = "locus:plugin";
    /// API majors accepted: the current one, and the previous one once there
    /// is one (§15.3).
    pub const SUPPORTED_API_MAJORS: &[u64] = &[1];
}

#[cfg(not(target_arch = "wasm32"))]
pub mod archive;
#[cfg(not(target_arch = "wasm32"))]
pub mod export;
#[cfg(not(target_arch = "wasm32"))]
pub mod pack;
#[cfg(not(target_arch = "wasm32"))]
pub mod registry;
#[cfg(not(target_arch = "wasm32"))]
pub mod runner;
#[cfg(not(target_arch = "wasm32"))]
mod runtime;
#[cfg(not(target_arch = "wasm32"))]
pub mod validators;

pub use error::{CallError, CancelToken};
#[cfg(not(target_arch = "wasm32"))]
pub use runtime::{Host, Limits, LoadError, MAX_COMPONENT_BYTES, Plugin};
