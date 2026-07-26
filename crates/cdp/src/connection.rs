use crate::error::{CdpError, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

#[cfg(target_os = "macos")]
use crate::browser::dismiss_allow_debugging_dialog;
use tokio_tungstenite::tungstenite::Message;
use tracing;

static NEXT_CDP_ID: AtomicU64 = AtomicU64::new(1);

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
    write: mpsc::UnboundedSender<InternalMessage>,
    events: broadcast::Sender<CdpEvent>,
    #[allow(dead_code)]
    handle: JoinHandle<()>,
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection").finish_non_exhaustive()
    }
}

impl Connection {
    /// Connect to a CDP WebSocket endpoint.
    pub async fn connect(ws_url: &str) -> Result<Self> {
        // Start connecting in background
        let ws_url_owned = ws_url.to_owned();
        let connect_fut = tokio::spawn(async move { connect_async(&ws_url_owned).await });

        // Wait 600ms, then dismiss the Dia dialog (if on macOS).
        // The dialog appears during the WebSocket handshake, so we must
        // dismiss it *while* the connection is still in progress.
        #[cfg(target_os = "macos")]
        {
            tokio::time::sleep(Duration::from_millis(600)).await;
            dismiss_allow_debugging_dialog();
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
            write: write_tx,
            events: events_tx,
            handle,
        })
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
    pub async fn call(
        &self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
    ) -> Result<Value> {
        let id = NEXT_CDP_ID.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();

        let msg = InternalMessage::Call {
            id,
            method: method.to_string(),
            params,
            session_id: session_id.map(String::from),
            tx,
        };

        self.write
            .send(msg)
            .map_err(|_| CdpError::ConnectionFailed {
                detail: "background I/O task has terminated".into(),
            })?;

        tokio::time::timeout(Duration::from_secs(30), rx)
            .await
            .map_err(|_| CdpError::CdpCallFailed {
                method: method.to_string(),
                detail: "timeout waiting for response".into(),
            })? // -> Result<Result<Value, CdpError>, RecvError>
            .map_err(|_| CdpError::CdpCallFailed {
                method: method.to_string(),
                detail: "oneshot channel closed".into(),
            })? // -> Result<Value, CdpError>
    }

    /// Subscribe to all CDP events broadcast from the browser.
    pub fn event_rx(&self) -> broadcast::Receiver<CdpEvent> {
        self.events.subscribe()
    }

    /// Disconnect cleanly. Closes the command channel and waits for the
    /// background I/O task to finish.
    pub async fn close(self) {
        let Self {
            write,
            events: _,
            handle,
        } = self;
        drop(write); // Signals the background loop to exit
        let _ = handle.await;
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn test_cdp_response_parsing() {
        let response = json!({"id": 1, "result": {"value": "hello"}});
        assert_eq!(response["id"].as_u64(), Some(1));
        assert!(response.get("error").is_none());
        assert_eq!(response["result"]["value"].as_str(), Some("hello"));
    }

    #[test]
    fn test_cdp_error_response() {
        let response =
            json!({"id": 2, "error": {"code": -32000, "message": "Cannot find context"}});
        assert!(response.get("error").is_some());
        assert_eq!(
            response["error"]["message"].as_str(),
            Some("Cannot find context")
        );
    }

    #[test]
    fn test_cdp_event_has_no_id() {
        let event = json!({"method": "Page.frameStartedLoading", "params": {"frameId": "123"}});
        assert!(event.get("id").is_none());
        assert!(event.get("method").is_some());
        assert!(event.get("params").is_some());
    }

    #[test]
    fn test_cdp_event_with_session_id() {
        let event = json!({
            "method": "Runtime.consoleAPICalled",
            "params": {},
            "sessionId": "session-123"
        });
        assert!(event.get("id").is_none());
        assert_eq!(event["sessionId"].as_str(), Some("session-123"));
    }
}
