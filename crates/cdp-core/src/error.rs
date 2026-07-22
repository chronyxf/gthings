use thiserror::Error;

#[derive(Debug, Error)]
pub enum CdpError {
    #[error("WebSocket error: {0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),
    #[error("CDP error response: {code} - {message}")]
    ErrorResponse { code: i64, message: String },
    #[error("Timeout after {0}ms waiting for response")]
    Timeout(u64),
    #[error("Connection closed")]
    ConnectionClosed,
    #[error("Session not found: {0}")]
    SessionNotFound(String),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Other(String),
}
