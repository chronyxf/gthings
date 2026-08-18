use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::Value;
use tokio::sync::mpsc;

use super::io::InternalMessage;

pub(crate) static NEXT_CDP_ID: AtomicU64 = AtomicU64::new(1);

/// Maximum number of retry attempts for CDP calls on ConnectionFailed errors.
pub(crate) const MAX_CALL_RETRIES: u32 = 3;

/// Base delay (ms) for exponential backoff between CDP call retries.
pub(crate) const CALL_RETRY_BASE_DELAY_MS: u64 = 200;

/// WebSocket connection timeout (10 seconds).
pub(crate) const WS_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Error message for WS connection timeouts.
pub(crate) const WS_CONNECT_TIMEOUT_MSG: &str = "WebSocket connection timed out after 10s";

/// Overall connection timeout (30 seconds).
pub(crate) const CONNECTION_TIMEOUT: Duration = Duration::from_secs(30);

/// Bounded channel capacity for CDP commands and events.
///
/// Provides backpressure: if the I/O task falls behind, callers will queue
/// instead of letting memory grow unboundedly. 256 is generous enough for
/// most workflows while keeping memory bounded.
pub(crate) const CHANNEL_CAPACITY: usize = 256;

/// A CDP event received from the browser (no "id" field, has "method" field).
#[derive(Debug, Clone)]
pub struct CdpEvent {
    pub method: String,
    pub params: Value,
    pub session_id: Option<String>,
}

/// Send a CDP command without waiting for the response (fire-and-forget).
/// Used by background tasks that don't need the result.
///
/// Uses `try_send` (non-blocking) so the caller is never suspended.
/// If the channel is full (backpressure), the message is dropped and
/// a warning is logged rather than blocking the background task.
pub(crate) fn call_async(
    write: &mpsc::Sender<InternalMessage>,
    method: &str,
    params: Value,
    session_id: Option<String>,
) {
    let id = NEXT_CDP_ID.fetch_add(1, Ordering::Relaxed);
    let msg = InternalMessage {
        id,
        method: method.to_string(),
        params,
        session_id,
        tx: None,
    };
    if let Err(e) = write.try_send(msg) {
        match e {
            mpsc::error::TrySendError::Full(_) => {
                tracing::warn!("CDP command channel full, dropping {method}");
            }
            mpsc::error::TrySendError::Closed(_) => {
                tracing::warn!("CDP command channel closed, dropping {method}");
            }
        }
    }
}
