use std::collections::HashMap;

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::error::{CdpError, Result};

use super::Connection;
use super::codec::CdpEvent;

/// Internal bookkeeping for an in-flight CDP command.
pub(crate) struct PendingCall {
    method: String,
    tx: oneshot::Sender<Result<Value>>,
}

pub(crate) type PendingMap = HashMap<u64, PendingCall>;

/// Internal message sent from `Connection::call` to the background I/O task.
///
/// `tx` is `Some` for awaited calls (routed to the pending map) and `None`
/// for fire-and-forget commands that don't need a response.
pub(crate) struct InternalMessage {
    pub id: u64,
    pub method: String,
    pub params: Value,
    pub session_id: Option<String>,
    pub tx: Option<oneshot::Sender<Result<Value>>>,
}

/// Background I/O loop state: the command channel, WebSocket halves, pending
/// call map, and event broadcaster threaded through [`RunState::run`].
pub(crate) struct RunState {
    pub(crate) write_rx: mpsc::Receiver<InternalMessage>,
    pub(crate) ws_writer: futures_util::stream::SplitSink<
        WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
        Message,
    >,
    pub(crate) ws_reader:
        futures_util::stream::SplitStream<WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>>,
    pub(crate) pending: PendingMap,
    pub(crate) events: broadcast::Sender<CdpEvent>,
}

impl RunState {
    /// Spawn the I/O loop on the current runtime, returning the task handle.
    pub(crate) fn spawn(self) -> JoinHandle<()> {
        tokio::spawn(async move { self.run().await })
    }

    /// Run the I/O loop until the WebSocket disconnects or the command
    /// channel closes.
    pub(crate) async fn run(mut self) {
        loop {
            tokio::select! {
                // Outgoing CDP commands from Connection::call
                msg = self.write_rx.recv() => {
                    match msg {
                        Some(InternalMessage { id, method, params, session_id, tx }) => {
                            let cmd = Connection::build_cdp_command(id, &method, &params, session_id.as_deref());
                            let text = match serde_json::to_string(&cmd) {
                                Ok(t) => t,
                                Err(e) => {
                                    tracing::warn!("Failed to serialize CDP command: {e}");
                                    if let Some(tx) = tx {
                                        let _ = tx.send(Err(CdpError::Json(e)));
                                    }
                                    continue;
                                }
                            };
                            // Store pending before sending to avoid race. Fire-and-forget
                            // messages (tx == None) are not tracked.
                            if let Some(tx) = tx {
                                self.pending.insert(id, PendingCall { method, tx });
                            }
                            if let Err(e) = self.ws_writer.send(Message::Text(text)).await {
                                tracing::warn!("WS send error: {e}");
                                if let Some(pc) = self.pending.remove(&id) {
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
                msg = self.ws_reader.next() => {
                    match msg {
                        Some(Ok(Message::Text(text))) => {
                            if let Ok(value) = serde_json::from_str::<Value>(&text) {
                                Connection::dispatch_message(value, &mut self.pending, &self.events).await;
                            }
                        }
                        Some(Ok(Message::Close(frame))) => {
                            tracing::debug!("CDP WebSocket closed: {frame:?}");
                            break;
                        }
                        Some(Ok(Message::Binary(_))) => {
                            tracing::warn!("Unexpected CDP Binary frame received, ignoring");
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
        for (_, pc) in self.pending.drain() {
            let _ = pc.tx.send(Err(CdpError::ConnectionFailed {
                detail: "WebSocket connection closed".into(),
            }));
        }
    }
}

impl Connection {
    /// Route an incoming JSON message: either a response (has "id") or an event (has "method").
    /// Response routing (has "id") dispatches to the pending oneshot sender;
    /// event dispatch (has "method") broadcasts via the event channel.
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::codec::CHANNEL_CAPACITY;
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
        let (event_tx, _) = broadcast::channel::<CdpEvent>(CHANNEL_CAPACITY);

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
        let (event_tx, _) = broadcast::channel::<CdpEvent>(CHANNEL_CAPACITY);

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
        let (event_tx, _) = broadcast::channel::<CdpEvent>(CHANNEL_CAPACITY);

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
        let (event_tx, mut event_rx) = broadcast::channel::<CdpEvent>(CHANNEL_CAPACITY);

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
        let (event_tx, mut event_rx) = broadcast::channel::<CdpEvent>(CHANNEL_CAPACITY);

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
}
