use crate::connection::{CdpEvent, Connection};
use crate::error::{CdpError, Result};
use crate::tab::Tab;
use serde_json::Value;
use std::time::Duration;
use tokio::sync::broadcast;
use tracing;

/// High-level CDP session. Manages connection + tabs with event-driven lifecycle.
pub struct Session {
    conn: Connection,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session").finish_non_exhaustive()
    }
}

/// Wait for a specific CDP event on a pre-subscribed receiver.
/// Subscribe BEFORE the triggering action to avoid missing the event.
async fn wait_for_event(
    rx: &mut broadcast::Receiver<CdpEvent>,
    method: &str,
    predicate: impl Fn(&CdpEvent) -> bool + Send,
    timeout: Duration,
) -> Result<CdpEvent> {
    tokio::time::timeout(timeout, async move {
        loop {
            match rx.recv().await {
                Ok(event) if event.method.as_str() == method && predicate(&event) => {
                    return Ok(event);
                }
                Ok(_) => continue,
                Err(broadcast::error::RecvError::Closed) => {
                    return Err(CdpError::ConnectionFailed {
                        detail: "event channel closed while waiting".into(),
                    });
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("Event receiver lagged by {n} messages");
                    continue;
                }
            }
        }
    })
    .await
    .map_err(|_| CdpError::NavigationTimeout {
        url: "unknown".into(),
        timeout: timeout.as_secs(),
    })?
}

impl Session {
    /// Connect to browser via WebSocket URL
    pub async fn connect(ws_url: &str) -> Result<Self> {
        let conn = Connection::connect(ws_url).await?;
        Ok(Session { conn })
    }

    /// Create a new tab/page
    pub async fn create_tab(&self, url: &str) -> Result<Tab> {
        Tab::create(self, url).await
    }

    /// Evaluate JavaScript in a tab, return JSON result
    pub async fn evaluate(&self, tab: &Tab, js: &str) -> Result<Value> {
        tab.evaluate(self, js).await
    }

    /// Navigate to URL and wait for networkIdle lifecycle event
    pub async fn navigate(&self, tab: &Tab, url: &str) -> Result<()> {
        let conn = &self.conn;
        let sid = tab.session_id.as_deref();

        // 1. Enable Page events so we receive lifecycle events
        conn.call("Page.enable", serde_json::json!({}), sid).await?;

        // 1a. Enable lifecycle events (required for Chrome 144+)
        conn.call(
            "Page.setLifecycleEventsEnabled",
            serde_json::json!({"enabled": true}),
            sid,
        )
        .await?;

        // 2. Subscribe BEFORE navigation — don't miss the networkIdle event
        let mut rx = conn.event_rx();

        // 3. Start navigation
        conn.call("Page.navigate", serde_json::json!({"url": url}), sid)
            .await?;

        // 4. Wait for networkIdle using the pre-subscribed receiver
        let result = wait_for_event(
            &mut rx,
            "Page.lifecycleEvent",
            |evt| evt.params.get("name").and_then(|v| v.as_str()) == Some("networkIdle"),
            Duration::from_secs(10),
        )
        .await;

        // Fallback: if lifecycle event timed out, poll document.readyState
        match result {
            Ok(_) => {}
            Err(CdpError::NavigationTimeout { .. }) => {
                tracing::warn!("Lifecycle event timeout, falling back to readyState polling");
                // Poll document.readyState up to 5 more seconds
                for _ in 0..10 {
                    if let Ok(val) = conn
                        .call(
                            "Runtime.evaluate",
                            serde_json::json!({
                                "expression": "document.readyState",
                                "returnByValue": true
                            }),
                            sid,
                        )
                        .await
                    {
                        if val
                            .get("result")
                            .and_then(|r| r.get("value"))
                            .and_then(|v| v.as_str())
                            == Some("complete")
                        {
                            return Ok(());
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                return Err(CdpError::NavigationTimeout {
                    url: url.to_string(),
                    timeout: 15,
                });
            }
            Err(e) => return Err(e),
        }

        Ok(())
    }

    /// Wait for a CDP event matching method + predicate.
    ///
    /// ⚠️ Creates a NEW event subscription. Call BEFORE the action that
    /// triggers the event to avoid race conditions. For navigation, use
    /// [`navigate()`](Session::navigate) instead.
    pub async fn wait_for<F>(
        &self,
        method: &str,
        predicate: F,
        timeout: Duration,
    ) -> Result<CdpEvent>
    where
        F: Fn(&CdpEvent) -> bool + Send + 'static,
    {
        let mut rx = self.conn.event_rx();

        tokio::time::timeout(timeout, async move {
            loop {
                match rx.recv().await {
                    Ok(event) if event.method.as_str() == method && predicate(&event) => {
                        return Ok(event);
                    }
                    Ok(_) => continue,
                    Err(broadcast::error::RecvError::Closed) => {
                        return Err(CdpError::ConnectionFailed {
                            detail: "event channel closed while waiting".into(),
                        });
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("Event receiver lagged by {n} messages");
                        continue;
                    }
                }
            }
        })
        .await
        .map_err(|_| CdpError::CdpCallFailed {
            method: format!("wait_for({method})"),
            detail: format!("timeout after {timeout:?}"),
        })?
    }

    /// Close a tab
    pub async fn close_tab(&self, tab: Tab) -> Result<()> {
        tab.close(self).await
    }

    /// Disconnect from browser
    pub async fn disconnect(self) -> Result<()> {
        self.conn.close().await;
        Ok(())
    }

    /// Access the underlying Connection (for direct CDP calls)
    pub fn connection(&self) -> &Connection {
        &self.conn
    }
}
