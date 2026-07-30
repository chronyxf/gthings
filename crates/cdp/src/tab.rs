use crate::connection::CdpEvent;
use crate::error::{CdpError, Result};
use crate::session::Session;
use serde_json::{Value, json};
use std::time::Duration;

/// Represents a browser tab/page
#[derive(Debug, Clone)]
pub struct Tab {
    pub target_id: String,
    pub session_id: Option<String>,
}

impl Tab {
    /// Create a new tab via CDP.
    ///
    /// If `background` is `true`, the tab is created with `background: true`,
    /// meaning it will not steal focus and can operate invisibly.
    pub async fn create(session: &Session, url: &str, background: bool) -> Result<Self> {
        let conn = session.connection();
        let mut params = json!({ "url": url });
        if background {
            params["background"] = json!(true);
        }
        let result = conn.call("Target.createTarget", params, None).await?;

        // Try to get sessionId first (standard CDP behavior)
        if let Some(session_id) = result.get("sessionId").and_then(|v| v.as_str()) {
            let target_id = result
                .get("targetId")
                .and_then(|v| v.as_str())
                .ok_or_else(|| CdpError::CdpCallFailed {
                    method: "Target.createTarget".into(),
                    detail: "no targetId in response".into(),
                })?
                .to_string();

            tracing::debug!(
                "Created tab: target={target}, session={session_id}",
                target = target_id
            );

            return Ok(Tab {
                session_id: Some(session_id.to_string()),
                target_id,
            });
        }

        // Missing sessionId — Dia returns targetId without sessionId.
        // Attach to the target via CDP instead of falling back to HTTP.
        if let Some(target_id) = result.get("targetId").and_then(|v| v.as_str()) {
            tracing::warn!("Target.createTarget returned targetId without sessionId, attaching...");
            let attach = conn
                .call(
                    "Target.attachToTarget",
                    json!({
                        "targetId": target_id,
                        "flatten": true,
                    }),
                    None,
                )
                .await
                .map_err(|e| CdpError::CdpCallFailed {
                    method: "Target.attachToTarget".into(),
                    detail: format!("attach failed: {e}"),
                })?;

            let session_id = attach
                .get("sessionId")
                .and_then(|v| v.as_str())
                .ok_or_else(|| CdpError::CdpCallFailed {
                    method: "Target.attachToTarget".into(),
                    detail: "no sessionId in attach response".into(),
                })?
                .to_string();

            tracing::info!("Attached to created target: {target_id} (session={session_id})",);

            return Ok(Tab {
                session_id: Some(session_id),
                target_id: target_id.to_string(),
            });
        }

        Err(CdpError::CdpCallFailed {
            method: "Target.createTarget".into(),
            detail: "could not create tab: no targetId or sessionId in response".into(),
        })
    }

    /// Create a background tab (no window focus steal) at `about:blank`.
    ///
    /// Equivalent to `Tab::create(session, "about:blank", true)`.
    pub async fn create_background(session: &Session) -> Result<Self> {
        Self::create(session, "about:blank", true).await
    }

    /// Navigate to URL and wait for fully loaded. Delegates to Session::navigate.
    pub async fn navigate(&self, session: &Session, url: &str) -> Result<()> {
        session.navigate(self, url).await
    }

    /// Evaluate JS in tab context, return JSON result.
    pub async fn evaluate(&self, session: &Session, js: &str) -> Result<Value> {
        let conn = session.connection();
        let sid = self.session_id.as_deref();
        conn.call(
            "Runtime.evaluate",
            json!({
                "expression": js,
                "returnByValue": true,
                "awaitPromise": true,
                "timeout": 10000,
            }),
            sid,
        )
        .await
    }

    /// Wait until page is fully loaded using lifecycle events
    pub async fn wait_loaded(&self, session: &Session, timeout: Duration) -> Result<()> {
        let sid = self.session_id.clone();

        session
            .wait_for(
                "Page.lifecycleEvent",
                move |evt: &CdpEvent| {
                    // Check that the event belongs to our session (if we have one)
                    let session_match = match &sid {
                        Some(sid) => evt.session_id.as_deref() == Some(sid.as_str()),
                        None => true,
                    };
                    // Check lifecycle name is networkIdle
                    let name_match =
                        evt.params.get("name").and_then(|v| v.as_str()) == Some("networkIdle");
                    session_match && name_match
                },
                timeout,
            )
            .await?;

        Ok(())
    }

    /// Extract page title
    pub async fn title(&self, session: &Session) -> Result<String> {
        match self.evaluate(session, "document.title || ''").await {
            Ok(val) => {
                let title = val
                    .get("result")
                    .and_then(|r| r.get("value"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_else(|| {
                        tracing::warn!("failed to extract title");
                        ""
                    });
                Ok(title.to_string())
            }
            Err(e) => {
                tracing::warn!("failed to extract title: {e}");
                Ok(String::new())
            }
        }
    }

    /// Close the tab
    pub async fn close(self, session: &Session) -> Result<()> {
        let conn = session.connection();
        let sid = self.session_id.as_deref();

        // Best-effort: close via JS first (Dia needs this before CDP close)
        let _ = conn
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": "window.close()",
                    "userGesture": true,
                }),
                sid,
            )
            .await;

        // Wait briefly for the JS close to take effect
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Then close via CDP
        conn.call(
            "Target.closeTarget",
            json!({ "targetId": self.target_id }),
            None,
        )
        .await?;
        Ok(())
    }
}
