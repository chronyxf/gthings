use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast;

use crate::connection::CdpConnection;
use crate::error::CdpError;
use crate::handler::CdpEvent;

/// A scoped CDP session attached to a specific target with flattened
/// session routing.
///
/// Wraps a `CdpConnection` and automatically injects `sessionId` into
/// every command. Also provides event subscription filtered to this
/// session's scope.
pub struct Session {
    conn: Arc<CdpConnection>,
    session_id: String,
    target_id: String,
}

impl Session {
    /// Attach to a CDP target using flattened sessions.
    ///
    /// Calls `Target.attachToTarget` with `flatten: true` and extracts
    /// the `sessionId` from the response.
    pub async fn attach(conn: &Arc<CdpConnection>, target_id: &str) -> Result<Self, CdpError> {
        let params = serde_json::json!({
            "targetId": target_id,
            "flatten": true,
        });
        let result = conn.call("Target.attachToTarget", Some(params)).await?;
        let session_id = result["sessionId"]
            .as_str()
            .ok_or_else(|| CdpError::Other("No sessionId in attach response".to_string()))?
            .to_string();
        Ok(Session {
            conn: conn.clone(),
            session_id,
            target_id: target_id.to_string(),
        })
    }

    /// Execute a command scoped to this session.
    pub async fn call(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, CdpError> {
        self.conn
            .call_with_session(&self.session_id, method, params)
            .await
    }

    /// Wait for a specific CDP event scoped to this session.
    ///
    /// Subscribes to the connection's event stream and filters by both
    /// the session ID and the event method name.
    pub async fn wait_for(&self, method: &str, timeout_ms: u64) -> Result<CdpEvent, CdpError> {
        let mut rx = self.conn.subscribe();
        let deadline = tokio::time::sleep(Duration::from_millis(timeout_ms));
        tokio::pin!(deadline);

        loop {
            tokio::select! {
                event = rx.recv() => {
                    match event {
                        Ok(evt) => {
                            if evt.method == method
                                && evt.session_id.as_deref() == Some(self.session_id.as_str())
                            {
                                return Ok(evt);
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!("Session event channel lagged by {n} messages");
                            continue;
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            return Err(CdpError::ConnectionClosed);
                        }
                    }
                }
                _ = &mut deadline => {
                    return Err(CdpError::Timeout(timeout_ms));
                }
            }
        }
    }

    /// The CDP target ID this session is attached to.
    pub fn target_id(&self) -> &str {
        &self.target_id
    }

    /// The CDP session ID used for scoped commands.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
}
