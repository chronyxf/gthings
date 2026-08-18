use super::{Tab, close_target_async};
use crate::connection::InternalMessage;
use crate::session::Session;
use tokio::sync::mpsc;

/// RAII guard that guarantees a tab is closed on ALL exit paths, including
/// task cancellation/abort.
///
/// Rust has no async `Drop`, so the guard cannot `await` the close. Instead it
/// holds a clone of the connection's write channel and, on `Drop`, fires a
/// non-blocking `Target.closeTarget` command via [`call_async`] (which uses
/// `try_send` and never suspends or blocks the runtime). The background I/O
/// task performs the actual close.
///
/// The guard is `Send` and may be moved across await points or into spawned
/// tasks. Dropping it (normally, on error, or on cancellation) closes the tab.
///
/// # Example
///
/// ```ignore
/// let tab = session.create_background_tab().await?;
/// let _guard = TabGuard::new(&session, tab); // closes tab on drop
/// ```
pub struct TabGuard {
    target_id: String,
    write: mpsc::Sender<InternalMessage>,
}

impl TabGuard {
    /// Take ownership of `tab` and close it when the guard is dropped.
    pub fn new(session: &Session, tab: Tab) -> Self {
        TabGuard {
            target_id: tab.target_id,
            write: session.connection().write_tx(),
        }
    }
}

impl Drop for TabGuard {
    fn drop(&mut self) {
        // Fire-and-forget, non-blocking close. No await, no blocking.
        close_target_async(&self.write, &self.target_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tab_guard_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<TabGuard>();
    }

    #[test]
    fn test_tab_guard_closes_on_drop() {
        let (tx, mut rx) = mpsc::channel::<InternalMessage>(16);
        let guard = TabGuard {
            target_id: "target-1".into(),
            write: tx,
        };
        drop(guard);

        let msg = rx.try_recv().expect("close message should be sent on drop");
        let InternalMessage { method, params, .. } = msg;
        {
            assert_eq!(method, "Target.closeTarget");
            assert_eq!(
                params.get("targetId").and_then(|v| v.as_str()),
                Some("target-1")
            );
        }
    }

    #[test]
    fn test_tab_guard_closes_on_cancellation() {
        // Simulate cancellation: the guard is dropped without an explicit
        // close (e.g. a JoinSet task is aborted mid-follow).
        let (tx, mut rx) = mpsc::channel::<InternalMessage>(16);
        {
            let _guard = TabGuard {
                target_id: "target-2".into(),
                write: tx,
            };
            // guard dropped at end of scope — no explicit close called
        }

        let msg = rx
            .try_recv()
            .expect("close message should be sent on cancellation drop");
        let InternalMessage { method, params, .. } = msg;
        {
            assert_eq!(method, "Target.closeTarget");
            assert_eq!(
                params.get("targetId").and_then(|v| v.as_str()),
                Some("target-2")
            );
        }
    }
}
