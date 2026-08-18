use crate::error::Result;
use crate::session::Session;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Shared, daemon-lifetime CDP session pool.
///
/// Holds exactly ONE warm [`Session`] (one browser WebSocket connection) for
/// the life of the daemon (PROPOSAL.md §7). The connection self-heals via the
/// underlying auto-reconnect in [`Connection::call`](crate::Connection).
pub struct SharedConnection {
    session: Arc<Session>,
    shutdown: AtomicBool,
    /// Port parsed from the WebSocket URL, used for diagnostics only.
    port: Option<u16>,
}

impl std::fmt::Debug for SharedConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedConnection")
            .field("shutdown", &self.shutdown.load(Ordering::SeqCst))
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

impl SharedConnection {
    /// Connect directly to a known CDP WebSocket endpoint.
    ///
    /// The port is parsed from the URL for diagnostics.
    pub async fn connect_ws(ws_url: &str) -> Result<Arc<Self>> {
        let session = Session::connect(ws_url, None).await?;
        let port = crate::connection::parse_port_from_ws_url(ws_url);
        Ok(Arc::new(Self::new(session, port)))
    }

    fn new(session: Session, port: Option<u16>) -> Self {
        Self {
            session: Arc::new(session),
            shutdown: AtomicBool::new(false),
            port,
        }
    }

    /// Clone of the warm session, for tab operations.
    pub fn session(&self) -> Arc<Session> {
        Arc::clone(&self.session)
    }

    /// Returns `true` once [`SharedConnection::shutdown`] has been called.
    pub fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }

    /// Graceful shutdown: mark the pool as shutting down so in-flight
    /// operations refuse to reconnect.
    ///
    /// The caller drops the last `Arc<SharedConnection>` afterwards, which
    /// drops the warm [`Session`] and aborts the WebSocket I/O task (sending a
    /// close frame to the browser). Idempotent.
    pub async fn shutdown(&self) {
        if self.shutdown.swap(true, Ordering::SeqCst) {
            return;
        }
        tracing::info!("shared connection shutdown");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shared_connection_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SharedConnection>();
        assert_send_sync::<Arc<SharedConnection>>();
    }
}
