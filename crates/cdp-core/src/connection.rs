use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use dashmap::DashMap;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

use crate::error::CdpError;
use crate::handler::{CallId, CdpEvent, CdpRequest, CdpResponse, MessageHandler};

/// Manages a persistent WebSocket connection to a CDP endpoint.
///
/// Architecture:
/// - **Writer task**: reads `CdpRequest` values from an `mpsc` channel and
///   serializes them as JSON text frames over the WebSocket.
/// - **Reader task**: reads text frames from the WebSocket, parses them, and
///   dispatches responses (messages with an `id` field) to the matching
///   pending `oneshot` channel, and events (messages with a `method` field)
///   to the `broadcast` channel.
pub struct CdpConnection {
    next_id: AtomicU64,
    pending: Arc<DashMap<CallId, oneshot::Sender<CdpResponse>>>,
    events: broadcast::Sender<CdpEvent>,
    write_tx: Mutex<Option<mpsc::Sender<CdpRequest>>>,
}

impl CdpConnection {
    /// Connect to a CDP WebSocket endpoint and start the reader/writer tasks.
    ///
    /// Returns an `Arc<Self>` so the connection can be shared with `Session`
    /// and `Browser` handles.
    pub async fn connect(ws_url: &str) -> Result<Arc<Self>, CdpError> {
        let (ws_stream, _) = connect_async(ws_url).await?;
        let (write, read) = ws_stream.split();

        let pending: Arc<DashMap<CallId, oneshot::Sender<CdpResponse>>> = Arc::new(DashMap::new());
        let (event_tx, _) = broadcast::channel::<CdpEvent>(256);
        let (req_tx, mut req_rx) = mpsc::channel::<CdpRequest>(256);

        // Writer task: serializes queued requests to the WebSocket
        let mut ws_write = write;
        tokio::spawn(async move {
            while let Some(request) = req_rx.recv().await {
                let json = match serde_json::to_string(&request) {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::error!("Failed to serialize CDP request: {e}");
                        break;
                    }
                };
                if let Err(e) = ws_write.send(Message::Text(json.into())).await {
                    tracing::error!("WebSocket write error: {e}");
                    break;
                }
            }
            // Channel closed — close the WebSocket gracefully
            let _ = ws_write.close().await;
        });

        // Reader task: demux incoming frames by id (response) vs method (event)
        let reader_pending = pending.clone();
        let reader_events = event_tx.clone();
        tokio::spawn(async move {
            let mut ws_read = read;
            while let Some(msg) = ws_read.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        if let Err(e) =
                            MessageHandler::process_message(&text, &reader_pending, &reader_events)
                        {
                            tracing::warn!("Failed to process CDP message: {e}");
                        }
                    }
                    Ok(Message::Close(_)) => {
                        tracing::debug!("WebSocket close frame received");
                        break;
                    }
                    Err(e) => {
                        tracing::error!("WebSocket read error: {e}");
                        break;
                    }
                    _ => {}
                }
            }
            // Drain pending map so any waiters get ConnectionClosed
            reader_pending.clear();
        });

        let conn = Arc::new(CdpConnection {
            next_id: AtomicU64::new(1),
            pending,
            events: event_tx,
            write_tx: Mutex::new(Some(req_tx)),
        });

        Ok(conn)
    }

    /// Generate a unique call ID.
    fn next_id(&self) -> CallId {
        self.next_id.fetch_add(1, Ordering::SeqCst)
    }

    /// Call a CDP method (no session scope).
    pub async fn call(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, CdpError> {
        self.call_inner(None, method, params).await
    }

    /// Call a CDP method with an explicit `session_id` for target-scoped
    /// commands (flattened session routing).
    pub async fn call_with_session(
        &self,
        session_id: &str,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, CdpError> {
        self.call_inner(Some(session_id.to_string()), method, params)
            .await
    }

    /// Shared inner implementation for `call` and `call_with_session`.
    async fn call_inner(
        &self,
        session_id: Option<String>,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, CdpError> {
        let id = self.next_id();
        let (tx, rx) = oneshot::channel();

        self.pending.insert(id, tx);

        let request = CdpRequest {
            id,
            session_id,
            method: method.to_string(),
            params,
        };

        // Clone the sender so we don't hold the lock across an await point
        let sender = {
            let guard = self.write_tx.lock().unwrap();
            guard.as_ref().ok_or(CdpError::ConnectionClosed)?.clone()
        };
        sender
            .send(request)
            .await
            .map_err(|_| CdpError::ConnectionClosed)?;

        // Wait for the response with a 30-second timeout
        let response = tokio::time::timeout(Duration::from_secs(30), rx)
            .await
            .map_err(|_| CdpError::Timeout(30_000))?
            .map_err(|_| CdpError::ConnectionClosed)?;

        match response.result {
            Ok(value) => Ok(value),
            Err(err) => Err(CdpError::ErrorResponse {
                code: err.code,
                message: err.message,
            }),
        }
    }

    /// Subscribe to all CDP events from this connection.
    pub fn subscribe(&self) -> broadcast::Receiver<CdpEvent> {
        self.events.subscribe()
    }

    /// Close the connection gracefully.
    ///
    /// Drops the write channel sender, which signals the writer task to close
    /// the WebSocket. The reader task will then exit when it sees the
    /// connection close.
    pub async fn close(&self) -> Result<(), CdpError> {
        let mut guard = self.write_tx.lock().unwrap();
        if let Some(tx) = guard.take() {
            drop(tx);
        }
        Ok(())
    }
}

/// Discover Chrome DevTools Protocol WebSocket URL from HTTP endpoint.
/// Tries /json/version first, falls back to /json, then /json/list.
pub async fn discover_ws_url(port: u16) -> Result<String, common::GthingsError> {
    // 1. Try /json/version (standard, works for Chrome, Edge, Brave)
    let url = format!("http://127.0.0.1:{port}/json/version");
    match reqwest::get(&url).await {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(json) = resp.json::<serde_json::Value>().await {
                if let Some(ws_url) = json["webSocketDebuggerUrl"].as_str() {
                    return Ok(ws_url.to_string());
                }
            }
        }
        _ => {}
    }

    // 2. Try /json (Dia Browser returns 404 on /json/version but works on /json)
    let url = format!("http://127.0.0.1:{port}/json");
    match reqwest::get(&url).await {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(targets) = resp.json::<Vec<serde_json::Value>>().await {
                // Find the browser target (type: "browser") or first page target
                for target in &targets {
                    if target["type"].as_str() == Some("browser") {
                        if let Some(ws_url) = target["webSocketDebuggerUrl"].as_str() {
                            return Ok(ws_url.to_string());
                        }
                    }
                }
                // Fallback: use first target's webSocketDebuggerUrl
                if let Some(first) = targets.first() {
                    if let Some(ws_url) = first["webSocketDebuggerUrl"].as_str() {
                        return Ok(ws_url.to_string());
                    }
                }
            }
        }
        _ => {}
    }

    // 3. Try /json/list as last resort
    let url = format!("http://127.0.0.1:{port}/json/list");
    match reqwest::get(&url).await {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(targets) = resp.json::<Vec<serde_json::Value>>().await {
                if let Some(first) = targets.first() {
                    if let Some(ws_url) = first["webSocketDebuggerUrl"].as_str() {
                        return Ok(ws_url.to_string());
                    }
                }
            }
        }
        _ => {}
    }

    Err(common::GthingsError::BrowserNotFound(port))
}

/// Connect directly to a CDP WebSocket URL (bypass HTTP discovery).
pub async fn connect_ws(ws_url: &str) -> Result<Arc<CdpConnection>, super::error::CdpError> {
    CdpConnection::connect(ws_url).await
}
