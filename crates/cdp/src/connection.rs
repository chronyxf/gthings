use crate::browser;
use crate::error::{CdpError, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

#[cfg(target_os = "macos")]
use crate::browser::dismiss_allow_debugging_dialog;
use tokio_tungstenite::tungstenite::Message;
pub(crate) static NEXT_CDP_ID: AtomicU64 = AtomicU64::new(1);

/// Maximum number of retry attempts for CDP calls on ConnectionFailed errors.
const MAX_CALL_RETRIES: u32 = 3;

/// Base delay (ms) for exponential backoff between CDP call retries.
const CALL_RETRY_BASE_DELAY_MS: u64 = 200;

/// WebSocket connection timeout (10 seconds).
const WS_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Overall connection timeout (15 seconds).
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(30);

/// Internal mutable state wrapped in `Arc<Mutex<...>>` so it can be
/// replaced atomically during auto-reconnect without changing the public
/// `&self` signature of [`Connection::call`].
struct ConnectionInner {
    write: mpsc::UnboundedSender<InternalMessage>,
    handle: JoinHandle<()>,
}

/// A CDP event received from the browser (no "id" field, has "method" field).
#[derive(Debug, Clone)]
pub struct CdpEvent {
    pub method: String,
    pub params: Value,
    pub session_id: Option<String>,
}

/// Internal bookkeeping for an in-flight CDP command.
pub(crate) struct PendingCall {
    method: String,
    tx: oneshot::Sender<Result<Value>>,
}

pub(crate) type PendingMap = HashMap<u64, PendingCall>;

/// Internal messages sent from `Connection::call` to the background I/O task.
pub(crate) enum InternalMessage {
    Call {
        id: u64,
        method: String,
        params: Value,
        session_id: Option<String>,
        tx: oneshot::Sender<Result<Value>>,
    },
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

impl Connection {
    /// Connect to a CDP WebSocket endpoint.
    ///
    /// `timeout` controls the per-call response timeout (default 30s when
    /// `None` is passed).
    pub async fn connect(ws_url: &str, timeout: Option<Duration>) -> Result<Self> {
        let timeout = timeout.unwrap_or(Duration::from_secs(30));

        tokio::time::timeout(CONNECTION_TIMEOUT, async {
            // Start connecting in background
            let ws_url_owned = ws_url.to_owned();
            let connect_fut = tokio::spawn(async move {
                match tokio::time::timeout(WS_CONNECT_TIMEOUT, connect_async(&ws_url_owned)).await {
                    Ok(Ok(pair)) => Ok(pair),
                    Ok(Err(e)) => Err(e),
                    Err(_) => Err(tokio_tungstenite::tungstenite::Error::Io(
                        std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            "WebSocket connection timed out after 10s",
                        ),
                    )),
                }
            });

            // Wait 600ms, then dismiss the Dia dialog (if on macOS).
            // The dialog appears during the WebSocket handshake, so we must
            // dismiss it *while* the connection is still in progress.
            #[cfg(target_os = "macos")]
            {
                tokio::time::sleep(Duration::from_millis(600)).await;
                dismiss_allow_debugging_dialog().await;
            }

            // Await the connection
            let (ws_stream, _) = connect_fut
                .await
                .map_err(|e| CdpError::ConnectionFailed {
                    detail: format!("task join: {e}"),
                })?
                .map_err(|e| CdpError::ConnectionFailed {
                    detail: format!("WebSocket connect to {ws_url} failed: {e}"),
                })?;

            let (write_tx, write_rx) = mpsc::unbounded_channel::<InternalMessage>();
            let (events_tx, _) = broadcast::channel::<CdpEvent>(256);
            let pending: PendingMap = HashMap::new();

            let (ws_writer, ws_reader) = ws_stream.split();
            let events_clone = events_tx.clone();

            let handle = tokio::spawn(async move {
                Self::run(write_rx, ws_writer, ws_reader, pending, events_clone).await;
            });

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
            detail: "Connection timed out after 15s".into(),
        })?
    }

    /// Background I/O loop: processes outgoing commands and incoming WebSocket messages.
    async fn run(
        mut write_rx: mpsc::UnboundedReceiver<InternalMessage>,
        mut ws_writer: futures_util::stream::SplitSink<
            WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
            Message,
        >,
        mut ws_reader: futures_util::stream::SplitStream<
            WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
        >,
        mut pending: PendingMap,
        events: broadcast::Sender<CdpEvent>,
    ) {
        loop {
            tokio::select! {
                // Outgoing CDP commands from Connection::call
                msg = write_rx.recv() => {
                    match msg {
                        Some(InternalMessage::Call { id, method, params, session_id, tx }) => {
                            let cmd = if let Some(ref sid) = session_id {
                                serde_json::json!({
                                    "id": id,
                                    "method": method,
                                    "params": params,
                                    "sessionId": sid,
                                })
                            } else {
                                serde_json::json!({
                                    "id": id,
                                    "method": method,
                                    "params": params,
                                })
                            };
                            let text = match serde_json::to_string(&cmd) {
                                Ok(t) => t,
                                Err(e) => {
                                    tracing::warn!("Failed to serialize CDP command: {e}");
                                    let _ = tx.send(Err(CdpError::Json(e)));
                                    continue;
                                }
                            };
                            // Store pending before sending to avoid race
                            pending.insert(id, PendingCall { method, tx });
                            if let Err(e) = ws_writer.send(Message::Text(text)).await {
                                tracing::warn!("WS send error: {e}");
                                if let Some(pc) = pending.remove(&id) {
                                    let _ = pc.tx.send(Err(CdpError::ConnectionFailed {
                                        detail: format!("WebSocket send failed: {e}"),
                                    }));
                                }
                                break;
                            }
                        }
                        None => break,
                    }
                }
                // Incoming WebSocket messages
                msg = ws_reader.next() => {
                    match msg {
                        Some(Ok(Message::Text(text))) => {
                            if let Ok(value) = serde_json::from_str::<Value>(&text) {
                                Self::dispatch_message(value, &mut pending, &events).await;
                            }
                        }
                        Some(Ok(Message::Close(frame))) => {
                            tracing::debug!("CDP WebSocket closed: {frame:?}");
                            break;
                        }
                        Some(Ok(Message::Binary(_))) => {
                            // Binary frames not expected from CDP
                        }
                        Some(Err(e)) => {
                            tracing::warn!("CDP WebSocket read error: {e}");
                            break;
                        }
                        None => {
                            tracing::debug!("CDP WebSocket stream ended");
                            break;
                        }
                        _ => {}
                    }
                }
            }
        }

        // Clean up all pending calls when the loop exits
        for (_, pc) in pending.drain() {
            let _ = pc.tx.send(Err(CdpError::ConnectionFailed {
                detail: "WebSocket connection closed".into(),
            }));
        }
    }

    /// Route an incoming JSON message: either a response (has "id") or an event (has "method").
    async fn dispatch_message(
        value: Value,
        pending: &mut PendingMap,
        events: &broadcast::Sender<CdpEvent>,
    ) {
        if let Some(id) = value.get("id").and_then(|v| v.as_u64()) {
            // Command response — route to the waiting oneshot
            if let Some(pc) = pending.remove(&id) {
                let result = if let Some(err) = value.get("error") {
                    let detail = err
                        .get("message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown error")
                        .to_string();
                    Err(CdpError::CdpCallFailed {
                        method: pc.method,
                        detail,
                    })
                } else {
                    Ok(value.get("result").cloned().unwrap_or(Value::Null))
                };
                let _ = pc.tx.send(result);
            }
        } else if let Some(method) = value.get("method").and_then(|v| v.as_str()) {
            // CDP event — broadcast to subscribers
            let session_id = value
                .get("sessionId")
                .and_then(|v| v.as_str())
                .map(String::from);
            let evt = CdpEvent {
                method: method.to_string(),
                params: value.get("params").cloned().unwrap_or(Value::Null),
                session_id,
            };
            let _ = events.send(evt);
        }
    }

    /// Send a CDP command and wait for the response.
    ///
    /// If `session_id` is `Some`, the command is sent as a Target-scoped message;
    /// if `None`, it is sent as a Browser-level message.
    /// Send a CDP command and wait for the response, with retry on
    /// ConnectionFailed errors using exponential backoff.
    pub async fn call(
        &self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
    ) -> Result<Value> {
        for attempt in 0..=MAX_CALL_RETRIES {
            let id = NEXT_CDP_ID.fetch_add(1, Ordering::Relaxed);
            let (tx, rx) = oneshot::channel();
            let msg = InternalMessage::Call {
                id,
                method: method.to_string(),
                params: params.clone(),
                session_id: session_id.map(String::from),
                tx,
            };
            match self.try_send(msg).await {
                Ok(()) => match self.await_response(rx, method).await {
                    Ok(v) => return Ok(v),
                    Err(CdpError::ConnectionFailed { .. }) => {
                        if attempt < MAX_CALL_RETRIES {
                            let delay = CALL_RETRY_BASE_DELAY_MS * 2_u64.pow(attempt);
                            tracing::warn!(
                                "CDP call {} failed (ConnectionFailed), retry {} in {}ms",
                                method,
                                attempt + 1,
                                delay
                            );
                            tokio::time::sleep(Duration::from_millis(delay)).await;
                            if let Err(e) = self.reconnect().await {
                                tracing::warn!("Reconnect attempt {} failed: {e}", attempt + 1);
                            }
                            continue;
                        }
                        return Err(CdpError::CdpCallFailed {
                            method: method.to_string(),
                            detail: format!(
                                "connection dropped after {} retries",
                                MAX_CALL_RETRIES
                            ),
                        });
                    }
                    Err(e) => return Err(e),
                },
                Err(CdpError::ConnectionFailed { .. }) => {
                    if attempt < MAX_CALL_RETRIES {
                        let delay = CALL_RETRY_BASE_DELAY_MS * 2_u64.pow(attempt);
                        tracing::warn!(
                            "CDP call {} failed (ConnectionFailed), retry {} in {}ms",
                            method,
                            attempt + 1,
                            delay
                        );
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                        if let Err(e) = self.reconnect().await {
                            tracing::warn!("Reconnect attempt {} failed: {e}", attempt + 1);
                        }
                        continue;
                    }
                    return Err(CdpError::ConnectionFailed {
                        detail: format!(
                            "call failed after {} retries: I/O loop terminated",
                            MAX_CALL_RETRIES
                        ),
                    });
                }
                Err(e) => return Err(e),
            }
        }

        Err(CdpError::CdpCallFailed {
            method: method.to_string(),
            detail: "all retry attempts exhausted".into(),
        })
    }

    /// Send a CDP command and wait for its response (single attempt, no reconnect).
    async fn try_send(&self, msg: InternalMessage) -> Result<()> {
        let inner = self.inner.lock().expect("connection inner lock poisoned");
        inner
            .write
            .send(msg)
            .map_err(|_| CdpError::ConnectionFailed {
                detail: "background I/O task has terminated".into(),
            })
    }

    /// Wait for a CDP response with the configured timeout.
    async fn await_response(
        &self,
        rx: oneshot::Receiver<Result<Value>>,
        method: &str,
    ) -> Result<Value> {
        tokio::time::timeout(self.timeout, rx)
            .await
            .map_err(|_| CdpError::CdpCallFailed {
                method: method.to_string(),
                detail: format!("timeout waiting for response ({}s)", self.timeout.as_secs()),
            })? // Result<Result<Value, CdpError>, RecvError>
            .map_err(|_| CdpError::CdpCallFailed {
                method: method.to_string(),
                detail: "oneshot channel closed".into(),
            })? // -> Result<Value, CdpError>
    }

    /// Re-detect the browser WebSocket endpoint and establish a new CDP
    /// connection, replacing the background I/O loop.
    ///
    /// Uses the same 5-strategy detection cascade as [`browser::detect`]:
    /// `GTHINGS_CDP_WS_URL` env var → `/json/version` → `/json` → `/json/list`
    /// → `DevToolsActivePort`. Falls back to the stored WebSocket URL if no
    /// port can be parsed.
    async fn reconnect(&self) -> Result<()> {
        let ws_url = self.resolve_ws_url().await?;

        let (ws_stream, _) = tokio::time::timeout(WS_CONNECT_TIMEOUT, connect_async(&ws_url))
            .await
            .map_err(|_| CdpError::ConnectionFailed {
                detail: "WebSocket connection timed out after 10s".into(),
            })?
            .map_err(|e| CdpError::ConnectionFailed {
                detail: format!("WebSocket reconnect failed: {e}"),
            })?;

        let (write_tx, write_rx) = mpsc::unbounded_channel::<InternalMessage>();
        let pending: PendingMap = HashMap::new();
        let (ws_writer, ws_reader) = ws_stream.split();
        let events = self.events.clone();

        let handle = tokio::spawn(async move {
            Self::run(write_rx, ws_writer, ws_reader, pending, events).await;
        });

        // Atomically replace the inner state (write channel + handle).
        let mut inner = self.inner.lock().expect("connection inner lock poisoned");
        inner.write = write_tx;
        inner.handle = handle;

        Ok(())
    }

    /// Resolve the WebSocket URL to use for reconnection, using the
    /// 5-strategy cascade from [`browser::detect`].
    async fn resolve_ws_url(&self) -> Result<String> {
        // 1. Environment variable bypass.
        if let Ok(url) = std::env::var("GTHINGS_CDP_WS_URL") {
            if !url.is_empty() {
                tracing::info!("reconnect: using GTHINGS_CDP_WS_URL env var");
                return Ok(url);
            }
        }

        // 2–5. Try browser detection cascade via port.
        if let Some(port) = parse_port_from_ws_url(&self.ws_url) {
            if let Ok(browser) = browser::detect(port).await {
                tracing::info!("reconnect: detected browser via cascade");
                return Ok(browser.ws_url);
            }
        }

        // Fallback: reconnect to the same URL.
        tracing::info!("reconnect: falling back to stored ws_url");
        Ok(self.ws_url.clone())
    }

    /// Subscribe to all CDP events broadcast from the browser.
    pub fn event_rx(&self) -> broadcast::Receiver<CdpEvent> {
        self.events.subscribe()
    }

    /// (crate-internal) Clone the write channel for fire-and-forget CDP commands
    /// from spawned background tasks.
    pub(crate) fn write_tx(&self) -> mpsc::UnboundedSender<InternalMessage> {
        self.inner
            .lock()
            .expect("connection inner lock poisoned")
            .write
            .clone()
    }

    /// Disconnect cleanly. Closes the command channel and waits for the
    /// background I/O task to finish.
    pub async fn close(self) {
        // We own self, so try_unwrap should succeed (Connection is not Clone).
        match Arc::try_unwrap(self.inner.clone()) {
            Ok(mutex) => {
                let inner = mutex.into_inner().unwrap_or_else(|e| {
                    tracing::warn!("Mutex poisoned in close: {e}");
                    e.into_inner()
                });
                drop(inner.write); // Signals the background loop to exit
                let _ = inner.handle.await;
            }
            Err(arc) => {
                // Fallback: clone the write sender and drop the clone to
                // signal the run loop. This should never be reached.
                let guard = arc.lock().expect("connection inner lock poisoned");
                drop(guard.write.clone());
            }
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        // Abort the background I/O task to prevent it from
        // outliving the connection and holding stale resources.
        // The WebSocket will be dropped as part of the task's
        // local variables, which sends a close frame to the browser.
        let inner = self.inner.lock().expect("connection inner lock poisoned");
        inner.handle.abort();
    }
}

/// Extract the port from a `ws://host:port/path` URL.
fn parse_port_from_ws_url(ws_url: &str) -> Option<u16> {
    let without_scheme = ws_url.strip_prefix("ws://")?;
    let host_port = without_scheme.split('/').next()?;
    let port_str = host_port.split(':').nth(1)?;
    port_str.parse().ok()
}

/// Send a CDP command without waiting for the response (fire-and-forget).
/// Used by background tasks that don't need the result.
pub(crate) fn call_async(
    write: &mpsc::UnboundedSender<InternalMessage>,
    method: &str,
    params: Value,
    session_id: Option<String>,
) {
    let id = NEXT_CDP_ID.fetch_add(1, Ordering::Relaxed);
    let (tx, _) = oneshot::channel();
    let msg = InternalMessage::Call {
        id,
        method: method.to_string(),
        params,
        session_id,
        tx,
    };
    let _ = write.send(msg);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -----------------------------------------------------------------------
    // dispatch_message routing tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_dispatch_message_routes_response_to_oneshot() {
        let mut pending: PendingMap = HashMap::new();
        let (tx, mut rx) = oneshot::channel();
        pending.insert(
            1,
            PendingCall {
                method: "Test.method".into(),
                tx,
            },
        );
        let (event_tx, _) = broadcast::channel::<CdpEvent>(256);

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(Connection::dispatch_message(
            json!({"id": 1, "result": {"data": "hello"}}),
            &mut pending,
            &event_tx,
        ));

        assert!(pending.is_empty(), "pending call should be removed");
        let received = rx.try_recv().expect("oneshot should have been sent");
        assert!(received.is_ok(), "response should be Ok");
        assert_eq!(
            received.unwrap().get("data").and_then(|v| v.as_str()),
            Some("hello")
        );
    }

    #[test]
    fn test_dispatch_message_routes_error_to_oneshot() {
        let mut pending: PendingMap = HashMap::new();
        let (tx, mut rx) = oneshot::channel();
        pending.insert(
            42,
            PendingCall {
                method: "Test.method".into(),
                tx,
            },
        );
        let (event_tx, _) = broadcast::channel::<CdpEvent>(256);

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(Connection::dispatch_message(
            json!({"id": 42, "error": {"code": -32000, "message": "Cannot find context"}}),
            &mut pending,
            &event_tx,
        ));

        assert!(pending.is_empty(), "pending call should be removed");
        let received = rx.try_recv().expect("oneshot should have been sent");
        assert!(received.is_err(), "error response should be Err");
    }

    #[test]
    fn test_dispatch_message_unknown_id_ignored() {
        let mut pending: PendingMap = HashMap::new();
        let (event_tx, _) = broadcast::channel::<CdpEvent>(256);

        // A response with an id not in pending should be silently ignored
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(Connection::dispatch_message(
            json!({"id": 999, "result": {}}),
            &mut pending,
            &event_tx,
        ));

        assert!(pending.is_empty(), "pending should remain empty");
    }

    #[test]
    fn test_dispatch_message_broadcasts_event_with_session_id() {
        let mut pending: PendingMap = HashMap::new();
        let (event_tx, mut event_rx) = broadcast::channel::<CdpEvent>(256);

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(Connection::dispatch_message(
            json!({"method": "Runtime.consoleAPICalled", "params": {"level": "info"}, "sessionId": "sess-1"}),
            &mut pending,
            &event_tx,
        ));

        assert!(pending.is_empty());
        let evt = event_rx.try_recv().expect("event should be broadcast");
        assert_eq!(evt.method, "Runtime.consoleAPICalled");
        assert_eq!(
            evt.params.get("level").and_then(|v| v.as_str()),
            Some("info")
        );
        assert_eq!(evt.session_id.as_deref(), Some("sess-1"));
    }

    #[test]
    fn test_dispatch_message_unrecognized_message_is_noop() {
        let mut pending: PendingMap = HashMap::new();
        let (event_tx, mut event_rx) = broadcast::channel::<CdpEvent>(256);

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(Connection::dispatch_message(
            json!({"some": "garbage"}),
            &mut pending,
            &event_tx,
        ));

        assert!(pending.is_empty());
        match event_rx.try_recv() {
            Err(broadcast::error::TryRecvError::Empty) => {} // expected
            other => panic!("expected Empty, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // reconnect / self-healing tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_reconnect_after_ws_drop() {
        // Start a local WebSocket echo server.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let ws_url = format!("ws://{}/devtools/browser/test", addr);

        // Server handle so we can kill it cleanly.
        let server_handle: Arc<Mutex<Option<JoinHandle<()>>>> = Arc::new(Mutex::new(None));
        let server_handle_clone = server_handle.clone();

        let server_task = tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                if let Ok(ws_stream) = tokio_tungstenite::accept_async(stream).await {
                    let (mut writer, mut reader) = ws_stream.split();
                    while let Some(Ok(msg)) = reader.next().await {
                        if let Message::Text(text) = msg {
                            if let Ok(value) = serde_json::from_str::<Value>(&text) {
                                if let Some(id) = value.get("id").and_then(|v| v.as_u64()) {
                                    let resp = json!({"id": id, "result": {"ok": true}});
                                    let _ = writer.send(Message::Text(resp.to_string())).await;
                                }
                            }
                        }
                    }
                }
            }
        });
        *server_handle_clone
            .lock()
            .expect("connection inner lock poisoned") = Some(server_task);

        // Small delay to ensure the server is listening.
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Connect.
        let conn = Connection::connect(&ws_url, None)
            .await
            .expect("connect should succeed");

        // Make a successful call.
        let result = conn.call("Test.echo", json!({"msg": "hello"}), None).await;
        assert!(result.is_ok(), "first call should succeed");

        // Kill the echo server — the TCP connection drops, the run loop
        // detects the WS error and exits.
        let handle = server_handle
            .lock()
            .expect("connection inner lock poisoned")
            .take();
        if let Some(h) = handle {
            h.abort(); // aborts the server task
        }
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Wait for the run loop to detect the disconnect and clean up.
        tokio::time::sleep(Duration::from_millis(300)).await;

        // The next call should see a dead write channel → ConnectionFailed.
        let result = conn.call("Test.echo", json!({"msg": "world"}), None).await;
        assert!(
            result.is_err(),
            "call after WS drop should fail: {result:?}"
        );

        // Start a *new* server on the same address.
        // Uses an accept loop so multiple connections (detect-cascade probes
        // followed by the actual WS reconnect) are all handled.
        let listener2 = tokio::net::TcpListener::bind(addr).await.unwrap();
        let server_task2 = tokio::spawn(async move {
            while let Ok((stream, _)) = listener2.accept().await {
                tokio::spawn(async move {
                    if let Ok(ws_stream) = tokio_tungstenite::accept_async(stream).await {
                        let (mut writer, mut reader) = ws_stream.split();
                        while let Some(Ok(msg)) = reader.next().await {
                            if let Message::Text(text) = msg {
                                if let Ok(value) = serde_json::from_str::<Value>(&text) {
                                    if let Some(id) = value.get("id").and_then(|v| v.as_u64()) {
                                        let resp = json!({"id": id, "result": {"ok": true}});
                                        let _ = writer.send(Message::Text(resp.to_string())).await;
                                    }
                                }
                            }
                        }
                    }
                });
            }
        });

        // Give the new server time to start.
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Reconnect (re-detection falls back to the stored ws_url).
        let reconnect_result = conn.reconnect().await;
        assert!(
            reconnect_result.is_ok(),
            "reconnect should succeed: {reconnect_result:?}"
        );

        // Make a call on the fresh connection.
        let result = conn
            .call("Test.echo", json!({"msg": "reconnected"}), None)
            .await;
        assert!(
            result.is_ok(),
            "call after reconnect should succeed: {result:?}"
        );

        // Clean up.
        server_task2.abort();
    }

    /// Spawn a CDP echo server that accepts multiple connections.
    /// Each connection handles CDP commands and responds with
    /// `{"id": <id>, "result": {"ok": true}}`.
    async fn spawn_echo_server(listener: tokio::net::TcpListener) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    if let Ok(ws_stream) = tokio_tungstenite::accept_async(stream).await {
                        let (mut writer, mut reader) = ws_stream.split();
                        while let Some(Ok(msg)) = reader.next().await {
                            if let Message::Text(text) = msg {
                                if let Ok(value) = serde_json::from_str::<Value>(&text) {
                                    if let Some(id) = value.get("id").and_then(|v| v.as_u64()) {
                                        let resp = json!({"id": id, "result": {"ok": true}});
                                        let _ = writer.send(Message::Text(resp.to_string())).await;
                                    }
                                }
                            }
                        }
                    }
                });
            }
        })
    }

    #[tokio::test]
    async fn test_reconnect_preserves_pending_calls() {
        // Start first CDP echo server
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let ws_url = format!("ws://{}/devtools/browser/test", addr);

        let server = spawn_echo_server(listener).await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        let conn = Connection::connect(&ws_url, None)
            .await
            .expect("connect should succeed");

        // Verify the connection works initially
        let result = conn.call("Test.echo", json!({"msg": "first"}), None).await;
        assert!(result.is_ok(), "first call should succeed: {result:?}");

        // Kill the echo server — the run loop detects the WS close and exits,
        // draining pending calls with ConnectionFailed errors.
        server.abort();
        tokio::time::sleep(Duration::from_millis(300)).await;

        // Start a new echo server on the same address
        let listener2 = tokio::net::TcpListener::bind(addr).await.unwrap();
        let server2 = spawn_echo_server(listener2).await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        // The call() method should detect the dead connection (try_send fails or
        // the oneshot returns ConnectionFailed), trigger reconnect_and_retry which
        // connects to the new server, and retry the CDP command — all transparently.
        let result = conn
            .call("Test.echo", json!({"msg": "after-reconnect"}), None)
            .await;
        assert!(
            result.is_ok(),
            "call after auto-reconnect should succeed: {result:?}"
        );

        server2.abort();
    }

    #[tokio::test]
    async fn test_reconnect_fails_after_max_attempts() {
        // Use a single-connection server so abort() closes the WS immediately.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let ws_url = format!("ws://{}/devtools/browser/test", addr);

        let server = tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                if let Ok(ws_stream) = tokio_tungstenite::accept_async(stream).await {
                    let (mut writer, mut reader) = ws_stream.split();
                    while let Some(Ok(msg)) = reader.next().await {
                        if let Message::Text(text) = msg {
                            if let Ok(value) = serde_json::from_str::<Value>(&text) {
                                if let Some(id) = value.get("id").and_then(|v| v.as_u64()) {
                                    let resp = json!({"id": id, "result": {"ok": true}});
                                    let _ = writer.send(Message::Text(resp.to_string())).await;
                                }
                            }
                        }
                    }
                }
            }
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let conn = Connection::connect(&ws_url, None)
            .await
            .expect("connect should succeed");

        // Verify initial connection works
        let result = conn.call("Test.echo", json!({"msg": "hello"}), None).await;
        assert!(result.is_ok(), "first call should succeed: {result:?}");

        // Abort the server — the TCP connection drops, the run loop detects
        // the WS close event, drains pending calls with ConnectionFailed, and
        // exits.
        server.abort();
        tokio::time::sleep(Duration::from_millis(300)).await;

        // The call should detect the dead connection, try reconnect_and_retry
        // which reconnects (fails — no server available), and return a
        // ConnectionFailed error.  MAX_RECONNECT_ATTEMPTS = 1 means only one
        // reconnect attempt is made — no retry loop.
        let result = conn.call("Test.echo", json!({"msg": "fail"}), None).await;
        match result {
            Err(CdpError::ConnectionFailed { detail }) => {
                assert!(
                    detail.contains("reconnect")
                        || detail.contains("WebSocket")
                        || detail.contains("I/O loop terminated"),
                    "error should mention reconnect failure, got: {detail}"
                );
            }
            other => panic!("expected ConnectionFailed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_reconnect_doesnt_loop_infinitely() {
        // Single-connection server, aborted to force WS disconnect.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let ws_url = format!("ws://{}/devtools/browser/test", addr);

        let server = tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                if let Ok(ws_stream) = tokio_tungstenite::accept_async(stream).await {
                    let (mut writer, mut reader) = ws_stream.split();
                    while let Some(Ok(msg)) = reader.next().await {
                        if let Message::Text(text) = msg {
                            if let Ok(value) = serde_json::from_str::<Value>(&text) {
                                if let Some(id) = value.get("id").and_then(|v| v.as_u64()) {
                                    let resp = json!({"id": id, "result": {"ok": true}});
                                    let _ = writer.send(Message::Text(resp.to_string())).await;
                                }
                            }
                        }
                    }
                }
            }
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let conn = Connection::connect(&ws_url, None)
            .await
            .expect("connect should succeed");

        let result = conn.call("Test.echo", json!({"msg": "hello"}), None).await;
        assert!(result.is_ok(), "first call should succeed: {result:?}");

        // Abort the server — WS connection drops, run loop exits.
        server.abort();
        tokio::time::sleep(Duration::from_millis(300)).await;

        // First post-disconnect call — reconnect attempt fails (no server),
        // returns error.  If there were an infinite loop, this would hang.
        let result1 = conn
            .call("Test.echo", json!({"msg": "attempt1"}), None)
            .await;
        assert!(
            result1.is_err(),
            "first post-disconnect call should fail: {result1:?}"
        );

        // Second post-disconnect call — should also fail cleanly.  If the
        // first attempt left the connection in a looping state, this would
        // hang or panic.
        let result2 = conn
            .call("Test.echo", json!({"msg": "attempt2"}), None)
            .await;
        assert!(
            result2.is_err(),
            "second post-disconnect call should fail: {result2:?}"
        );

        // Both errors must be ConnectionFailed (not panic or timeout).
        assert!(
            matches!(&result1, Err(CdpError::ConnectionFailed { .. })),
            "error should be ConnectionFailed: {result1:?}"
        );
        assert!(
            matches!(&result2, Err(CdpError::ConnectionFailed { .. })),
            "error should be ConnectionFailed: {result2:?}"
        );
    }
}
