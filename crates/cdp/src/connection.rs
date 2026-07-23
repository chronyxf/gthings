use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::{Mutex, oneshot};
use tokio_tungstenite::MaybeTlsStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tracing;

use crate::error::{CdpError, Result};

type PendingMap = Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>;

/// CDP WebSocket connection with oneshot command dispatch.
/// Each command is sent with a unique `id`, and the response is matched
/// by that id via a oneshot channel.
pub struct Connection {
    write: futures_util::stream::SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>,
    pending: PendingMap,
    next_id: AtomicU64,
}

impl Connection {
    /// Create a new CDP connection from a WebSocket stream.
    /// Spawns a background reader task that dispatches responses to pending oneshots.
    pub async fn new(
        ws_stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
        mut kill_rx: tokio::sync::oneshot::Receiver<()>,
    ) -> Result<Self> {
        let (write, read) = ws_stream.split();
        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let pending_clone = pending.clone();

        // Spawn reader task
        tokio::spawn(async move {
            let mut read = read;
            loop {
                tokio::select! {
                    msg = read.next() => {
                        match msg {
                            Some(Ok(Message::Text(text))) => {
                                // Parse response
                                if let Ok(value) = serde_json::from_str::<Value>(&text) {
                                    if let Some(id) = value.get("id").and_then(|v| v.as_u64()) {
                                        let mut map = pending_clone.lock().await;
                                        if let Some(tx) = map.remove(&id) {
                                            let _ = tx.send(value);
                                        }
                                    }
                                    // Events (no "id" field) are silently ignored
                                }
                            }
                            Some(Ok(Message::Binary(_))) => {
                                // Binary frames not expected from CDP
                            }
                            Some(Ok(Message::Close(frame))) => {
                                tracing::debug!("CDP WebSocket closed: {:?}", frame);
                                break;
                            }
                            Some(Err(e)) => {
                                tracing::warn!("CDP WebSocket error: {e}");
                                break;
                            }
                            None => {
                                tracing::debug!("CDP WebSocket stream ended");
                                break;
                            }
                            _ => {}
                        }
                    }
                    _ = &mut kill_rx => {
                        tracing::debug!("Kill signal received, stopping reader");
                        break;
                    }
                }
            }
        });

        Ok(Connection {
            write,
            pending,
            next_id: AtomicU64::new(1),
        })
    }

    /// Send a CDP command and wait for the response.
    /// Uses oneshot dispatch: sends the command with a unique id, registers
    /// the oneshot sender in the pending map, and awaits the response.
    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);

        let cmd = serde_json::json!({
            "id": id,
            "method": method,
            "params": params,
        });

        let text = serde_json::to_string(&cmd)?;

        let (tx, rx) = oneshot::channel();
        {
            let mut map = self.pending.lock().await;
            map.insert(id, tx);
        }

        self.write.send(Message::Text(text)).await?;

        // Wait for response with 30s timeout
        let response = tokio::time::timeout(Duration::from_secs(30), rx)
            .await
            .map_err(|_| CdpError::Timeout(30000))?
            .map_err(|_| CdpError::ChannelBroken)?;

        // Check for CDP error
        if let Some(err) = response.get("error") {
            let msg = err
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error")
                .to_string();
            return Err(CdpError::CommandFailed {
                method: method.to_string(),
                msg,
            });
        }

        Ok(response.get("result").cloned().unwrap_or(Value::Null))
    }

    /// Send a CDP command with an explicit sessionId (for tab-specific commands).
    pub async fn call_with_session(
        &mut self,
        session_id: &str,
        method: &str,
        params: Value,
    ) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);

        let cmd = serde_json::json!({
            "id": id,
            "method": method,
            "params": params,
            "sessionId": session_id,
        });

        let text = serde_json::to_string(&cmd)?;

        let (tx, rx) = oneshot::channel();
        {
            let mut map = self.pending.lock().await;
            map.insert(id, tx);
        }

        self.write.send(Message::Text(text)).await?;

        let response = tokio::time::timeout(Duration::from_secs(30), rx)
            .await
            .map_err(|_| CdpError::Timeout(30000))?
            .map_err(|_| CdpError::ChannelBroken)?;

        if let Some(err) = response.get("error") {
            let msg = err
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error")
                .to_string();
            return Err(CdpError::CommandFailed {
                method: method.to_string(),
                msg,
            });
        }

        Ok(response.get("result").cloned().unwrap_or(Value::Null))
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
}
