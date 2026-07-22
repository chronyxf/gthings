use dashmap::DashMap;
use tokio::sync::{broadcast, oneshot};

use crate::error::CdpError;

/// A CDP message identifier, used to match requests to responses.
pub type CallId = u64;

/// A request sent to the CDP endpoint.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CdpRequest {
    pub id: CallId,
    #[serde(skip_serializing_if = "Option::is_none", rename = "sessionId")]
    pub session_id: Option<String>,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

/// The result of a single CDP command, matched by id.
#[derive(Debug, Clone)]
pub struct CdpResponse {
    pub id: CallId,
    pub session_id: Option<String>,
    pub result: Result<serde_json::Value, CdpErrorBody>,
}

/// Error body from a CDP error response.
#[derive(Debug, Clone)]
pub struct CdpErrorBody {
    pub code: i64,
    pub message: String,
}

/// An event emitted by the browser (no id field, has method field).
#[derive(Debug, Clone)]
pub struct CdpEvent {
    pub session_id: Option<String>,
    pub method: String,
    pub params: serde_json::Value,
}

/// Routes incoming WebSocket messages to the appropriate handler:
/// responses (has "id") → pending oneshot channels,
/// events (has "method") → broadcast channel.
pub struct MessageHandler;

impl MessageHandler {
    /// Parse a raw CDP WebSocket text frame and dispatch it.
    pub fn process_message(
        text: &str,
        pending: &DashMap<CallId, oneshot::Sender<CdpResponse>>,
        events: &broadcast::Sender<CdpEvent>,
    ) -> Result<(), CdpError> {
        let value: serde_json::Value = serde_json::from_str(text)?;

        if let Some(id) = value.get("id").and_then(|v| v.as_u64()) {
            // --- Command response ---
            let result = if let Some(error) = value.get("error") {
                let code = error.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
                let message = error
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown error")
                    .to_string();
                Err(CdpErrorBody { code, message })
            } else {
                Ok(value
                    .get("result")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null))
            };

            let session_id = value
                .get("sessionId")
                .and_then(|v| v.as_str().map(String::from));
            let response = CdpResponse {
                id,
                session_id,
                result,
            };
            if let Some((_, sender)) = pending.remove(&id) {
                let _ = sender.send(response);
            }
        } else if value.get("method").is_some() {
            // --- Event ---
            let event = CdpEvent {
                session_id: value
                    .get("sessionId")
                    .and_then(|v| v.as_str().map(String::from)),
                method: value
                    .get("method")
                    .and_then(|v| v.as_str().map(String::from))
                    .unwrap_or_default(),
                params: value
                    .get("params")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            };
            let _ = events.send(event);
        }

        Ok(())
    }
}
