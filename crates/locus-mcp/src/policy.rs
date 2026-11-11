// SPDX-License-Identifier: AGPL-3.0-or-later

//! Scopes, approval modes, rate limits and payload caps (SPEC §16.4).
//!
//! Pure policy, decided here and nowhere else, so every transport enforces
//! the same rules and the security tests (§16.5) test one place.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::error::ToolError;

/// What a tool needs (SPEC §16.3 table).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Read,
    Write,
    Export,
}

impl Scope {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Scope::Read => "read",
            Scope::Write => "write",
            Scope::Export => "export",
        }
    }
}

/// When the user is asked before a write (SPEC §16.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMode {
    /// Ask before every write call. The default: the safest choice.
    #[default]
    AskEachWrite,
    /// Ask once; after a yes, writes go through until the MCP session ends.
    AskPerSession,
    /// Never ask for writes.
    Allow,
}

/// The user's grants for one MCP session. Default: read only (SPEC §16.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grants {
    pub read: bool,
    pub write: bool,
    pub export: bool,
    pub approval: ApprovalMode,
    /// Export always asks unless the user explicitly allowed it (§16.4).
    pub export_without_asking: bool,
}

impl Default for Grants {
    fn default() -> Self {
        Self {
            read: true,
            write: false,
            export: false,
            approval: ApprovalMode::AskEachWrite,
            export_without_asking: false,
        }
    }
}

impl Grants {
    /// Everything granted and nothing asked: what a user means by passing
    /// `--scopes read,write,export` to the headless CLI, where there is no
    /// one to ask (the scopes on the command line *are* the approval).
    #[must_use]
    pub fn headless(read: bool, write: bool, export: bool) -> Self {
        Self {
            read,
            write,
            export,
            approval: ApprovalMode::Allow,
            export_without_asking: true,
        }
    }

    #[must_use]
    pub fn has(&self, scope: Scope) -> bool {
        match scope {
            Scope::Read => self.read,
            Scope::Write => self.write,
            Scope::Export => self.export,
        }
    }
}

/// What the server must do before running a tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// Run it.
    Go,
    /// Ask the user first; on a yes call [`Policy::approved`].
    Ask,
}

/// Per-session policy state: grants plus what the user already approved.
#[derive(Debug, Clone, Default)]
pub struct Policy {
    grants: Grants,
    session_approved: bool,
}

impl Policy {
    #[must_use]
    pub fn new(grants: Grants) -> Self {
        Self {
            grants,
            session_approved: false,
        }
    }

    #[must_use]
    pub fn grants(&self) -> Grants {
        self.grants
    }

    /// Replace the grants (the user changed them in the app). Changing the
    /// approval mode forgets an earlier "yes for this session".
    pub fn set_grants(&mut self, grants: Grants) {
        if grants.approval != self.grants.approval || !grants.write {
            self.session_approved = false;
        }
        self.grants = grants;
    }

    /// Whether `tool` (needing `scope`) may run, and whether to ask first.
    ///
    /// # Errors
    /// [`ToolError::ScopeDenied`] when the scope is not granted.
    pub fn check(&self, tool: &str, scope: Scope) -> Result<Gate, ToolError> {
        if !self.grants.has(scope) {
            return Err(ToolError::ScopeDenied {
                tool: tool.to_owned(),
                scope: scope.name(),
            });
        }
        Ok(match scope {
            Scope::Read => Gate::Go,
            Scope::Export if self.grants.export_without_asking => Gate::Go,
            Scope::Export => Gate::Ask,
            Scope::Write => match self.grants.approval {
                ApprovalMode::Allow => Gate::Go,
                ApprovalMode::AskPerSession if self.session_approved => Gate::Go,
                ApprovalMode::AskPerSession | ApprovalMode::AskEachWrite => Gate::Ask,
            },
        })
    }

    /// The user said yes to a prompt for `scope`.
    pub fn approved(&mut self, scope: Scope) {
        if scope == Scope::Write && self.grants.approval == ApprovalMode::AskPerSession {
            self.session_approved = true;
        }
    }
}

// ── Caps (SPEC §16.4: payload size caps on write tools) ─────────────────────

/// Largest tool-call arguments accepted, as JSON bytes.
pub const MAX_ARGS_BYTES: usize = 256 * 1024;
/// Most items (entities, constraints, IDs) in one write call. Pattern
/// instance limits (§6.2.1) are enforced by the document itself.
pub const MAX_ITEMS: usize = 2_000;
/// Largest HTTP request body (a little over [`MAX_ARGS_BYTES`] for framing).
pub const MAX_BODY_BYTES: usize = MAX_ARGS_BYTES + 16 * 1024;

/// # Errors
/// [`ToolError::TooLarge`] when `got` is over `limit`.
pub fn cap(what: &'static str, got: usize, limit: usize) -> Result<(), ToolError> {
    if got > limit {
        Err(ToolError::TooLarge { what, limit, got })
    } else {
        Ok(())
    }
}

// ── Rate limiting (SPEC §16.4) ──────────────────────────────────────────────

/// A sliding-window limit on write and export calls.
#[derive(Debug, Clone)]
pub struct RateLimiter {
    window: Duration,
    max: usize,
    recent: VecDeque<Instant>,
}

impl Default for RateLimiter {
    /// 30 calls per 10 seconds: far above a human-paced conversation, far
    /// below what would flood the undo history.
    fn default() -> Self {
        Self::new(30, Duration::from_secs(10))
    }
}

impl RateLimiter {
    #[must_use]
    pub fn new(max: usize, window: Duration) -> Self {
        Self {
            window,
            max,
            recent: VecDeque::new(),
        }
    }

    /// Count one call at `now`.
    ///
    /// # Errors
    /// [`ToolError::RateLimited`] when the window is full; the call is not counted.
    pub fn hit(&mut self, now: Instant) -> Result<(), ToolError> {
        while let Some(&t) = self.recent.front() {
            if now.saturating_duration_since(t) >= self.window {
                self.recent.pop_front();
            } else {
                break;
            }
        }
        if self.recent.len() >= self.max {
            let oldest = self.recent.front().copied().unwrap_or(now);
            let wait = self
                .window
                .saturating_sub(now.saturating_duration_since(oldest));
            return Err(ToolError::RateLimited {
                retry_after_ms: u64::try_from(wait.as_millis()).unwrap_or(u64::MAX),
            });
        }
        self.recent.push_back(now);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_grant_is_read_only() {
        let p = Policy::default();
        assert_eq!(p.check("get_entity", Scope::Read), Ok(Gate::Go));
        assert!(matches!(
            p.check("add_entities", Scope::Write),
            Err(ToolError::ScopeDenied { scope: "write", .. })
        ));
        assert!(matches!(
            p.check("export", Scope::Export),
            Err(ToolError::ScopeDenied {
                scope: "export",
                ..
            })
        ));
    }

    #[test]
    fn read_can_be_withheld() {
        let p = Policy::new(Grants {
            read: false,
            ..Grants::default()
        });
        assert!(p.check("get_entity", Scope::Read).is_err());
    }

    #[test]
    fn ask_each_write_always_asks() {
        let mut p = Policy::new(Grants {
            write: true,
            ..Grants::default()
        });
        assert_eq!(p.check("t", Scope::Write), Ok(Gate::Ask));
        p.approved(Scope::Write);
        assert_eq!(p.check("t", Scope::Write), Ok(Gate::Ask));
    }

    #[test]
    fn ask_per_session_asks_once() {
        let mut p = Policy::new(Grants {
            write: true,
            approval: ApprovalMode::AskPerSession,
            ..Grants::default()
        });
        assert_eq!(p.check("t", Scope::Write), Ok(Gate::Ask));
        p.approved(Scope::Write);
        assert_eq!(p.check("t", Scope::Write), Ok(Gate::Go));
        // Changing the mode forgets the yes.
        p.set_grants(Grants {
            write: true,
            approval: ApprovalMode::AskEachWrite,
            ..Grants::default()
        });
        p.set_grants(Grants {
            write: true,
            approval: ApprovalMode::AskPerSession,
            ..Grants::default()
        });
        assert_eq!(p.check("t", Scope::Write), Ok(Gate::Ask));
    }

    #[test]
    fn allow_never_asks_but_export_still_does() {
        let mut p = Policy::new(Grants {
            write: true,
            export: true,
            approval: ApprovalMode::Allow,
            ..Grants::default()
        });
        assert_eq!(p.check("t", Scope::Write), Ok(Gate::Go));
        assert_eq!(p.check("export", Scope::Export), Ok(Gate::Ask));
        p.approved(Scope::Export);
        assert_eq!(p.check("export", Scope::Export), Ok(Gate::Ask));
        p.set_grants(Grants {
            export_without_asking: true,
            ..p.grants()
        });
        assert_eq!(p.check("export", Scope::Export), Ok(Gate::Go));
    }

    #[test]
    fn rate_limiter_window() {
        let mut r = RateLimiter::new(2, Duration::from_secs(10));
        let t0 = Instant::now();
        assert!(r.hit(t0).is_ok());
        assert!(r.hit(t0).is_ok());
        let Err(ToolError::RateLimited { retry_after_ms }) = r.hit(t0 + Duration::from_secs(1))
        else {
            panic!("expected a rate limit");
        };
        assert_eq!(retry_after_ms, 9_000);
        assert!(r.hit(t0 + Duration::from_secs(10)).is_ok());
    }

    #[test]
    fn caps() {
        assert!(cap("x", 3, 3).is_ok());
        assert_eq!(
            cap("x", 4, 3),
            Err(ToolError::TooLarge {
                what: "x",
                limit: 3,
                got: 4
            })
        );
    }
}
