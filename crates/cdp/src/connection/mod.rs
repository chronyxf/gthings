//! CDP WebSocket transport: event-driven connection, command call/retry,
//! self-healing reconnection, macOS dialog handling, and wire encoding.
//!
//! Submodule layout:
//! - [`codec`] owns wire items: `CdpEvent`, call constants, `NEXT_CDP_ID`,
//!   and the fire-and-forget `call_async`.
//! - [`io`] owns the background I/O loop state and message dispatch.
//! - [`retry`] owns `Connection::call` retry/backoff logic.
//! - [`reconnect`] owns the auto-reconnect cascade.
//! - [`dialog`] owns the socket-first + osascript dialog dismissal fallback.

mod codec;
mod dialog;
mod io;
mod reconnect;
mod retry;

pub use self::codec::CdpEvent;
pub(crate) use self::codec::call_async;
pub(crate) use self::io::InternalMessage;
pub(crate) use self::reconnect::parse_port_from_ws_url;

use self::codec::{
    CHANNEL_CAPACITY, CONNECTION_TIMEOUT, WS_CONNECT_TIMEOUT, WS_CONNECT_TIMEOUT_MSG,
};
use self::io::{PendingMap, RunState};
use crate::error::{CdpError, Result};
use futures_util::StreamExt;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

/// Default per-call CDP response timeout when the caller passes `None`.
const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// Connect to a CDP WebSocket endpoint with the shared [`WS_CONNECT_TIMEOUT`]
/// bound, mapping timeout and handshake failures into [`CdpError`].
///
/// `what` is a human-readable label used in the failure detail (e.g.
/// "WebSocket connect to ws://…" or "WebSocket reconnect").
async fn connect_ws(
    url: &str,
    what: &str,
) -> Result<WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>> {
    tokio::time::timeout(WS_CONNECT_TIMEOUT, connect_async(url))
        .await
        .map_err(|_| CdpError::ConnectionFailed {
            detail: WS_CONNECT_TIMEOUT_MSG.into(),
        })?
        .map(|(stream, _response)| stream)
        .map_err(|e| CdpError::ConnectionFailed {
            detail: format!("{what} failed: {e}"),
        })
}

/// Internal mutable state wrapped in `Arc<Mutex<...>>` so it can be
/// replaced atomically during auto-reconnect without changing the public
/// `&self` signature of [`Connection::call`].
///
/// Uses a **bounded** channel (capacity [`CHANNEL_CAPACITY`]) to provide
/// backpressure: if the background I/O task cannot keep up, callers will
/// block or receive an error instead of unbounded memory growth.
struct ConnectionInner {
    write: mpsc::Sender<InternalMessage>,
    handle: JoinHandle<()>,
}

/// Event-driven CDP WebSocket connection.
///
/// Spawns a background task that multiplexes outgoing CDP commands and
/// incoming messages (responses → oneshot dispatch, events → broadcast).
pub struct Connection {
    inner: Arc<Mutex<ConnectionInner>>,
    events: broadcast::Sender<CdpEvent>,
    ws_url: String,
    timeout: Duration,
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection").finish_non_exhaustive()
    }
}

/// Acquire a lock, recovering from a poisoned mutex with a warning.
fn recover_poisoned_lock<'a, T>(mutex: &'a Mutex<T>, what: &str) -> MutexGuard<'a, T> {
    mutex.lock().unwrap_or_else(|e| {
        tracing::warn!(target: "gthings_cdp", "{what} mutex poisoned, recovering");
        e.into_inner()
    })
}

impl Connection {
    /// Lock the inner state, recovering from a poisoned mutex if needed.
    fn lock_inner(&self) -> MutexGuard<'_, ConnectionInner> {
        recover_poisoned_lock(&self.inner, "connection")
    }

    /// Build a CDP command JSON value, optionally including a sessionId.
    fn build_cdp_command(id: u64, method: &str, params: &Value, session_id: Option<&str>) -> Value {
        let mut cmd = serde_json::json!({
            "id": id,
            "method": method,
            "params": params,
        });
        if let Some(sid) = session_id {
            cmd["sessionId"] = serde_json::json!(sid);
        }
        cmd
    }

    /// Connect to a CDP WebSocket endpoint.
    ///
    /// `timeout` controls the per-call response timeout (default 30s when
    /// `None` is passed).
    pub async fn connect(ws_url: &str, timeout: Option<Duration>) -> Result<Self> {
        let timeout = timeout.unwrap_or(DEFAULT_CALL_TIMEOUT);

        tokio::time::timeout(CONNECTION_TIMEOUT, async {
            // Start connecting in background
            let ws_url_owned = ws_url.to_owned();
            let connect_fut = tokio::spawn(async move {
                connect_ws(
                    &ws_url_owned,
                    &format!("WebSocket connect to {ws_url_owned}"),
                )
                .await
            });

            let ws_stream = Self::connect_with_dialog(connect_fut, ws_url).await?;

            // Bounded channel limits in-flight commands and provides backpressure.
            let (write_tx, write_rx) = mpsc::channel::<InternalMessage>(CHANNEL_CAPACITY);
            let (events_tx, _) = broadcast::channel::<CdpEvent>(CHANNEL_CAPACITY);
            let pending: PendingMap = HashMap::new();

            let (ws_writer, ws_reader) = ws_stream.split();
            let events_clone = events_tx.clone();

            let handle = RunState {
                write_rx,
                ws_writer,
                ws_reader,
                pending,
                events: events_clone,
            }
            .spawn();

            Ok(Connection {
                inner: Arc::new(Mutex::new(ConnectionInner {
                    write: write_tx,
                    handle,
                })),
                events: events_tx,
                ws_url: ws_url.to_owned(),
                timeout,
            })
        })
        .await
        .map_err(|_| CdpError::ConnectionFailed {
            detail: format!(
                "Connection timed out after {}s",
                CONNECTION_TIMEOUT.as_secs()
            ),
        })?
    }

    /// Subscribe to all CDP events broadcast from the browser.
    pub fn event_rx(&self) -> broadcast::Receiver<CdpEvent> {
        self.events.subscribe()
    }

    /// (crate-internal) Clone the write channel for fire-and-forget CDP commands
    /// from spawned background tasks.
    pub(crate) fn write_tx(&self) -> mpsc::Sender<InternalMessage> {
        self.lock_inner().write.clone()
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        // Abort the background I/O task to prevent it from
        // outliving the connection and holding stale resources.
        // The WebSocket will be dropped as part of the task's
        // local variables, which sends a close frame to the browser.
        self.lock_inner().handle.abort();
    }
}
