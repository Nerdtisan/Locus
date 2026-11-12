// SPDX-License-Identifier: AGPL-3.0-or-later

//! Why a plugin call failed, on every host (§15.4). Shared by the desktop
//! wasmtime runtime and the web core-module host (ADR 0016), so both report
//! the same errors.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::types::PluginFailure;
use crate::validate::Malformed;

/// Why a call produced no result. Every variant leaves the document unchanged.
#[derive(Debug, thiserror::Error)]
pub enum CallError {
    #[error("the plugin used more than its CPU budget")]
    CpuLimit,
    #[error("the plugin took longer than {0:?}")]
    Timeout(Duration),
    #[error("the plugin needed more than {} MB of memory", .0 / (1024 * 1024))]
    MemoryLimit(usize),
    /// A trap: a panic, `unreachable`, out-of-bounds access, stack overflow…
    #[error("the plugin crashed ({0})")]
    Trap(String),
    /// The plugin returned data that failed validation, or that the host
    /// couldn't even read (bad pointers, invalid UTF-8, …).
    #[error("the plugin returned malformed output: {0}")]
    Malformed(String),
    /// The plugin reported an error itself.
    #[error(transparent)]
    Plugin(PluginFailure),
    #[error("the plugin doesn't provide a {0}")]
    NotProvided(&'static str),
    #[error("plugin host error: {0}")]
    Host(String),
    /// Stopped through its [`CancelToken`] (#214).
    #[error("the plugin call was cancelled")]
    Cancelled,
}

/// Cancels a running call from another thread (#214). Cheap to clone; give
/// one to `Plugin::with_cancel`. The call stops at the next epoch tick
/// (10 ms) with [`CallError::Cancelled`] and returns nothing.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    #[must_use]
    pub fn new() -> CancelToken {
        CancelToken::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

impl From<Malformed> for CallError {
    fn from(m: Malformed) -> Self {
        CallError::Malformed(m.0)
    }
}
