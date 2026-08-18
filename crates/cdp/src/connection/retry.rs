use std::sync::atomic::Ordering;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::oneshot;

use crate::error::{CdpError, Result};

use super::Connection;
use super::codec::{CALL_RETRY_BASE_DELAY_MS, MAX_CALL_RETRIES, NEXT_CDP_ID};
use super::io::InternalMessage;

/// Outcome of a single "sleep + reconnect, or give up" retry step.
enum RetryOutcome {
    /// Retry the call after sleeping and attempting a reconnect.
    Retry,
    /// Retries exhausted — return the supplied error.
    GiveUp(CdpError),
}

impl Connection {
    /// Send a CDP command and wait for the response, with retry on
    /// ConnectionFailed errors using exponential backoff.
    ///
    /// If `session_id` is `Some`, the command is sent as a Target-scoped message;
    /// if `None`, it is sent as a Browser-level message.
    pub async fn call(
        &self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
    ) -> Result<Value> {
        // Hoist allocations out of the retry loop — method and session_id are
        // the same across attempts. params.clone() is still needed inside the
        // loop because `Value` is moved into `InternalMessage`; we must
        // retain the original to reconstruct the message on each retry.
        let method_owned = method.to_string();
        let session_id_owned = session_id.map(String::from);

        for attempt in 0..=MAX_CALL_RETRIES {
            let id = NEXT_CDP_ID.fetch_add(1, Ordering::Relaxed);
            let (tx, rx) = oneshot::channel();
            let msg = InternalMessage {
                id,
                method: method_owned.clone(),
                // params is cloned each retry because the original `Value` is
                // consumed by the first `InternalMessage` that is moved into
                // the channel.  We keep the original for subsequent attempts.
                params: params.clone(),
                session_id: session_id_owned.clone(),
                tx: Some(tx),
            };
            match self.try_send(msg).await {
                Ok(()) => match self.await_response(rx, method).await {
                    Ok(v) => return Ok(v),
                    Err(CdpError::ConnectionFailed { .. }) => {
                        match self
                            .retry_or_fail(attempt, method, || CdpError::CdpCallFailed {
                                method: method_owned.clone(),
                                detail: format!(
                                    "connection dropped after {} retries",
                                    MAX_CALL_RETRIES
                                ),
                            })
                            .await?
                        {
                            RetryOutcome::Retry => continue,
                            RetryOutcome::GiveUp(e) => return Err(e),
                        }
                    }
                    Err(e) => return Err(e),
                },
                Err(CdpError::ConnectionFailed { .. }) => {
                    match self
                        .retry_or_fail(attempt, method, || CdpError::ConnectionFailed {
                            detail: format!(
                                "call failed after {} retries: I/O loop terminated",
                                MAX_CALL_RETRIES
                            ),
                        })
                        .await?
                    {
                        RetryOutcome::Retry => continue,
                        RetryOutcome::GiveUp(e) => return Err(e),
                    }
                }
                Err(e) => return Err(e),
            }
        }

        Err(CdpError::CdpCallFailed {
            method: method_owned,
            detail: "all retry attempts exhausted".into(),
        })
    }

    /// One "sleep + reconnect, or fail" step shared by both ConnectionFailed
    /// branches of [`Connection::call`]. Returns [`RetryOutcome::Retry`] when
    /// an attempt remains (after sleeping and attempting a reconnect), or
    /// [`RetryOutcome::GiveUp`] with the caller-supplied error when retries
    /// are exhausted.
    async fn retry_or_fail(
        &self,
        attempt: u32,
        method: &str,
        give_up: impl FnOnce() -> CdpError,
    ) -> Result<RetryOutcome> {
        if attempt < MAX_CALL_RETRIES {
            let delay = CALL_RETRY_BASE_DELAY_MS * 2_u64.pow(attempt);
            tracing::warn!(
                "CDP call {method} failed (ConnectionFailed), retry {} in {delay}ms",
                attempt + 1
            );
            tokio::time::sleep(Duration::from_millis(delay)).await;
            if let Err(e) = self.reconnect().await {
                tracing::warn!("Reconnect attempt {} failed: {e}", attempt + 1);
            }
            Ok(RetryOutcome::Retry)
        } else {
            Ok(RetryOutcome::GiveUp(give_up()))
        }
    }

    /// Send a CDP command and wait for its response (single attempt, no reconnect).
    ///
    /// Uses the bounded channel's async `send()` which waits for capacity,
    /// providing natural backpressure. If the receiver (background I/O task)
    /// has dropped (i.e., the WebSocket disconnected), returns
    /// `CdpError::ConnectionFailed`.
    async fn try_send(&self, msg: InternalMessage) -> Result<()> {
        // Clone the sender while holding the lock, then drop the guard before
        // awaiting — `MutexGuard` is not `Send` and would violate the future's
        // `Send` bound.
        let sender = { self.lock_inner().write.clone() };
        sender
            .send(msg)
            .await
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
        let Ok(inner) = tokio::time::timeout(self.timeout, rx).await else {
            return Err(CdpError::CdpCallFailed {
                method: method.to_string(),
                detail: format!("timeout waiting for response ({}s)", self.timeout.as_secs()),
            });
        };
        let Ok(result) = inner else {
            return Err(CdpError::CdpCallFailed {
                method: method.to_string(),
                detail: "oneshot channel closed".into(),
            });
        };
        result
    }
}
