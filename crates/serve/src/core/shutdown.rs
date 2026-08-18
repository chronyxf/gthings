//! Graceful drain-on-SIGTERM/SIGINT shutdown.
//!
//! On a termination signal the daemon must stop accepting new jobs, let
//! in-flight jobs finish, close every live browser tab, and exit with
//! `128 + signum` so the parent process sees the conventional termination
//! status. [`drain_on_signal`] is the composition root for that sequence;
//! [`Shutdown`] is the cooperative "still accepting?" flag the HTTP layer
//! consults before enqueueing.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gthings_cdp::SharedConnection;
use gthings_common::telemetry::StderrEvent;
use tokio::signal::unix::{SignalKind, signal};

/// POSIX signal number for SIGTERM (graceful terminate).
pub(crate) const SIGTERM: i32 = 15;
/// POSIX signal number for SIGINT (Ctrl-C).
pub(crate) const SIGINT: i32 = 2;

/// How long in-flight jobs get to finish before the drain abandons them.
pub(crate) const DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Exit code for a process terminated by `signum`: `128 + signum`.
#[must_use]
pub(crate) const fn exit_code(signum: i32) -> i32 {
    128 + signum
}

/// Cooperative "still accepting jobs?" flag shared with the HTTP layer.
///
/// The daemon flips it off when a termination signal arrives so `POST /job`
/// answers 503 instead of enqueueing work that will never run.
#[derive(Debug, Clone)]
pub(crate) struct Shutdown {
    accepting: Arc<AtomicBool>,
}

impl Shutdown {
    /// Create a shutdown flag that starts in the accepting state.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            accepting: Arc::new(AtomicBool::new(true)),
        }
    }

    /// Whether the daemon should still accept new jobs.
    #[must_use]
    pub(crate) fn is_accepting(&self) -> bool {
        self.accepting.load(Ordering::SeqCst)
    }

    /// Stop accepting new jobs.
    pub(crate) fn begin(&self) {
        self.accepting.store(false, Ordering::SeqCst);
    }
}

impl Default for Shutdown {
    fn default() -> Self {
        Self::new()
    }
}

/// Wait for the next SIGTERM or SIGINT, returning its signal number.
pub(crate) async fn wait_for_signal() -> i32 {
    let mut term = signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");
    let mut intr = signal(SignalKind::interrupt()).expect("failed to install SIGINT handler");
    tokio::select! {
        _ = term.recv() => SIGTERM,
        _ = intr.recv() => SIGINT,
    }
}

/// Drain the daemon after a termination signal and return the exit code.
///
/// 1. Stops accepting new jobs (`shutdown.begin()`).
/// 2. Marks the shared connection as shutting down via [`SharedConnection::shutdown`].
/// 3. Returns `128 + signum` for the embedding process to exit with.
///
/// In-flight jobs are drained by the caller: the embedding process must drop
/// the [`JobQueue`](super::queue::JobQueue) send half first so the worker loop
/// ends once the buffer drains, then await the worker handle.
pub(crate) async fn drain_on_signal(
    shutdown: &Shutdown,
    connection: Option<Arc<SharedConnection>>,
) -> i32 {
    let signum = wait_for_signal().await;
    StderrEvent::new(
        "info",
        String::new(),
        serde_json::json!({"event": "drain-start", "signal": signum}),
    )
    .emit()
    .ok();
    shutdown.begin();

    if let Some(connection) = connection {
        connection.shutdown().await;
        StderrEvent::new(
            "info",
            String::new(),
            serde_json::json!({"event": "connection-shutdown"}),
        )
        .emit()
        .ok();
    }

    exit_code(signum)
}

#[cfg(test)]
mod tests {
    use super::Shutdown;

    #[test]
    fn shutdown_flag_starts_accepting_and_ends() {
        let shutdown = Shutdown::new();
        assert!(shutdown.is_accepting());
        shutdown.begin();
        assert!(!shutdown.is_accepting());
        shutdown.begin(); // idempotent
        assert!(!shutdown.is_accepting());
    }
}
