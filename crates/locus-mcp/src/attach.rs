// SPDX-License-Identifier: AGPL-3.0-or-later

//! Attaching the server to a GUI (SPEC §16.2, #229): tool jobs and approval
//! prompts are queued for the UI thread, which runs them in its frame loop
//! through its own `Sketcher`, so the user watches edits happen.
//!
//! `wake` is called whenever something is queued (the GUI requests a
//! repaint), so an idle app still answers promptly.

use std::collections::VecDeque;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use crate::host::{ApprovalRequest, Approver, Backend, Job, Workspace};

type Wake = Arc<dyn Fn() + Send + Sync>;

/// The server side of the job queue.
pub struct QueueBackend {
    tx: Mutex<mpsc::Sender<Job>>,
    wake: Wake,
}

impl std::fmt::Debug for QueueBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("QueueBackend")
    }
}

impl Backend for QueueBackend {
    fn submit(&self, job: Job) -> bool {
        let sent = self.tx.lock().is_ok_and(|tx| tx.send(job).is_ok());
        if sent {
            (self.wake)();
        }
        sent
    }
}

/// The UI side of the job queue.
#[derive(Debug)]
pub struct Jobs {
    rx: mpsc::Receiver<Job>,
}

impl Jobs {
    /// Run every queued job against `ws`; returns how many ran.
    pub fn drain(&self, ws: &mut dyn Workspace) -> usize {
        let mut n = 0;
        while let Ok(job) = self.rx.try_recv() {
            job(ws);
            n += 1;
        }
        n
    }
}

/// A job queue for a GUI thread.
#[must_use]
pub fn jobs(wake: impl Fn() + Send + Sync + 'static) -> (Arc<QueueBackend>, Jobs) {
    let (tx, rx) = mpsc::channel();
    (
        Arc::new(QueueBackend {
            tx: Mutex::new(tx),
            wake: Arc::new(wake),
        }),
        Jobs { rx },
    )
}

/// Approval prompts waiting for the user, oldest first.
pub struct Prompts {
    queue: Mutex<VecDeque<(ApprovalRequest, tokio::sync::oneshot::Sender<bool>)>>,
    wake: Wake,
}

impl std::fmt::Debug for Prompts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Prompts")
    }
}

impl Prompts {
    #[must_use]
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(Self {
            queue: Mutex::new(VecDeque::new()),
            wake: Arc::new(wake),
        })
    }

    /// The prompt to show now, if any. Prompts whose caller gave up (timed
    /// out or disconnected) are dropped.
    #[must_use]
    pub fn current(&self) -> Option<ApprovalRequest> {
        let mut q = self.queue.lock().ok()?;
        while q.front().is_some_and(|(_, tx)| tx.is_closed()) {
            q.pop_front();
        }
        q.front().map(|(r, _)| r.clone())
    }

    /// Answer the current prompt.
    pub fn answer(&self, yes: bool) {
        if let Some((_, tx)) = self.queue.lock().ok().and_then(|mut q| q.pop_front()) {
            let _ = tx.send(yes);
        }
    }

    /// Refuse everything pending (MCP was turned off).
    pub fn decline_all(&self) {
        if let Ok(mut q) = self.queue.lock() {
            for (_, tx) in q.drain(..) {
                let _ = tx.send(false);
            }
        }
    }
}

impl Approver for Prompts {
    fn ask(&self, request: ApprovalRequest, reply: tokio::sync::oneshot::Sender<bool>) {
        if let Ok(mut q) = self.queue.lock() {
            q.push_back((request, reply));
        }
        (self.wake)();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::Headless;
    use crate::policy::Scope;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn jobs_run_on_drain_and_wake_the_ui() {
        let woken = Arc::new(AtomicUsize::new(0));
        let w = woken.clone();
        let (backend, jobs) = jobs(move || {
            w.fetch_add(1, Ordering::SeqCst);
        });
        let (tx, rx) = std::sync::mpsc::channel();
        assert!(backend.submit(Box::new(move |ws| {
            let _ = tx.send(ws.sketcher().document().layers().len());
        })));
        assert_eq!(woken.load(Ordering::SeqCst), 1);
        assert!(rx.try_recv().is_err(), "nothing runs before the UI drains");
        let mut ws = Headless::new(locus_doc::Document::new());
        assert_eq!(jobs.drain(&mut ws), 1);
        assert_eq!(rx.try_recv().unwrap(), 1);
        drop(jobs);
        assert!(!backend.submit(Box::new(|_| {})), "UI gone");
    }

    #[test]
    fn prompts_queue_answer_and_decline() {
        let p = Prompts::new(|| {});
        let req = |s: &str| ApprovalRequest {
            tool: "t".into(),
            scope: Scope::Write,
            summary: s.into(),
        };
        let (tx1, mut rx1) = tokio::sync::oneshot::channel();
        let (tx2, mut rx2) = tokio::sync::oneshot::channel();
        let (tx3, rx3) = tokio::sync::oneshot::channel();
        p.ask(req("one"), tx1);
        p.ask(req("two"), tx2);
        assert_eq!(p.current().unwrap().summary, "one");
        p.answer(true);
        assert_eq!(rx1.try_recv(), Ok(true));
        assert_eq!(p.current().unwrap().summary, "two");
        p.ask(req("three"), tx3);
        drop(rx3);
        p.decline_all();
        assert_eq!(rx2.try_recv(), Ok(false));
        assert!(p.current().is_none());
    }

    #[test]
    fn abandoned_prompts_are_skipped() {
        let p = Prompts::new(|| {});
        let (tx, rx) = tokio::sync::oneshot::channel();
        p.ask(
            ApprovalRequest {
                tool: "t".into(),
                scope: Scope::Write,
                summary: "gone".into(),
            },
            tx,
        );
        drop(rx);
        assert!(p.current().is_none());
    }
}
