// SPDX-License-Identifier: AGPL-3.0-or-later
//! One plugin call on a background thread (SPEC §15.4, #214).
//!
//! The UI thread starts a [`Job`], polls it once a frame and applies the
//! result through the command API when it arrives, so a slow plugin never
//! freezes the app. Cancelling sets the job's [`CancelToken`]: the plugin
//! host stops the call at its next epoch tick, and the result (an error) is
//! simply dropped, so the document is never touched.

use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use locus_plugin_host::CancelToken;

/// A call in flight. Dropping it abandons the result.
#[derive(Debug)]
pub struct Job<T> {
    rx: Receiver<T>,
    cancel: CancelToken,
    started: Instant,
}

/// What [`Job::poll`] found.
#[derive(Debug, PartialEq)]
pub enum Poll<T> {
    Running,
    Done(T),
    /// The thread ended without an answer (it panicked).
    Lost,
}

impl<T: Send + 'static> Job<T> {
    /// Runs `work` on a new thread. It gets the job's token, to hand to
    /// `Plugin::with_cancel`.
    pub fn spawn(work: impl FnOnce(CancelToken) -> T + Send + 'static) -> Result<Job<T>, String> {
        let (tx, rx) = mpsc::channel();
        let cancel = CancelToken::new();
        let token = cancel.clone();
        std::thread::Builder::new()
            .name("locus-plugin-call".into())
            .spawn(move || {
                // The receiver may be gone (cancelled); nothing to do then.
                let _ = tx.send(work(token));
            })
            .map_err(|e| format!("couldn't start the plugin call ({})", e.kind()))?;
        Ok(Job {
            rx,
            cancel,
            started: Instant::now(),
        })
    }

    pub fn poll(&self) -> Poll<T> {
        match self.rx.try_recv() {
            Ok(v) => Poll::Done(v),
            Err(TryRecvError::Empty) => Poll::Running,
            Err(TryRecvError::Disconnected) => Poll::Lost,
        }
    }

    /// Asks the call to stop; the caller then drops the job.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn wait<T: Send + 'static>(job: &Job<T>) -> Poll<T> {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            match job.poll() {
                Poll::Running if Instant::now() < until => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                other => return other,
            }
        }
    }

    #[test]
    fn result_arrives_without_blocking_the_caller() {
        let gate = std::sync::Arc::new(std::sync::Barrier::new(2));
        let g = gate.clone();
        let job = Job::spawn(move |_| {
            g.wait();
            42
        })
        .unwrap();
        // Still running: the caller isn't blocked.
        assert_eq!(job.poll(), Poll::Running);
        gate.wait();
        assert_eq!(wait(&job), Poll::Done(42));
    }

    #[test]
    fn cancel_reaches_the_work() {
        let job = Job::spawn(|token: CancelToken| {
            while !token.is_cancelled() {
                std::thread::sleep(Duration::from_millis(1));
            }
            "stopped"
        })
        .unwrap();
        job.cancel();
        assert_eq!(wait(&job), Poll::Done("stopped"));
    }

    #[test]
    fn a_panicking_call_is_lost_not_fatal() {
        let job: Job<()> = Job::spawn(|_| panic!("plugin host bug")).unwrap();
        assert_eq!(wait(&job), Poll::Lost);
    }
}
