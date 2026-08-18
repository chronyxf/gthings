pub mod dialog;
pub mod navigate;
pub mod signals;
pub mod tabs;
pub mod user_agent;

use crate::connection::Connection;
use crate::error::Result;
use std::time::Duration;
use tokio::task::JoinHandle;

/// High-level CDP session. Manages connection + tabs with event-driven lifecycle.
pub struct Session {
    conn: Connection,
    /// Handle to the background dialog auto-accept task, aborted on drop.
    dialog_handle: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session").finish_non_exhaustive()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Abort the dialog handler task to prevent it from
        // outliving the session and holding a stale write channel.
        if let Some(handle) = self.dialog_handle.take() {
            handle.abort();
        }
    }
}

impl Session {
    /// Connect to browser via WebSocket URL.
    ///
    /// `timeout` controls the per-call CDP response timeout
    /// (defaults to 30 seconds when `None`).
    pub async fn connect(ws_url: &str, timeout: Option<Duration>) -> Result<Self> {
        let conn = Connection::connect(ws_url, timeout).await?;
        let dialog_handle = Some(Self::spawn_dialog_handler(&conn));
        Ok(Session {
            conn,
            dialog_handle,
        })
    }

    /// Access the underlying Connection (for direct CDP calls)
    pub fn connection(&self) -> &Connection {
        &self.conn
    }
}
