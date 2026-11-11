// SPDX-License-Identifier: AGPL-3.0-or-later

//! wasmtime-based plugin runtime, capability enforcement and plugin registry
//! (SPEC §15, ADR 0009). Desktop-only at first (SPEC §0): on `wasm32` only the
//! plain types and validation build, so the web build never links wasmtime.

pub mod types;
pub mod validate;

#[cfg(not(target_arch = "wasm32"))]
mod runtime;

#[cfg(not(target_arch = "wasm32"))]
pub use runtime::{
    API_PACKAGE, CallError, Host, Limits, LoadError, MAX_COMPONENT_BYTES, Plugin,
    SUPPORTED_API_MAJORS,
};
