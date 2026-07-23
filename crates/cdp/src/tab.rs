use crate::connection::Connection;
use crate::error::{CdpError, Result};
use serde_json::{Value, json};
use std::time::Duration;
use tracing;
use url::Url;

/// A CDP tab session, representing one browser tab.
pub struct Tab {
    /// The CDP session ID for this tab's target
    pub session_id: String,
    /// The target ID
    pub target_id: String,
}

impl Tab {
    /// Create a new tab by creating a CDP target that navigates to a URL.
    /// Uses `Target.createTarget` which returns a sessionId.
    /// Falls back to creating a target via HTTP `/json/new` endpoint (works on Dia)
    /// then attaching to it.
    pub async fn create(conn: &mut Connection, ws_url: &str, url: &str) -> Result<Self> {
        // First try to create a new target (standard CDP)
        let result = conn
            .call(
                "Target.createTarget",
                json!({
                    "url": url,
                    "newWindow": false,
                }),
            )
            .await;

        // If CDP succeeded and has a sessionId, use it directly
        if let Ok(result) = result {
            if let Some(session_id) = result.get("sessionId").and_then(|v| v.as_str()) {
                let target_id = result
                    .get("targetId")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| CdpError::CommandFailed {
                        method: "Target.createTarget".into(),
                        msg: "no targetId in response".into(),
                    })?
                    .to_string();

                tracing::debug!("Created tab: target={}, session={}", target_id, session_id);

                return Ok(Tab {
                    session_id: session_id.to_string(),
                    target_id,
                });
            }
            // Got targetId but no sessionId (e.g. reusing persistent browser) —
            // fall through to HTTP method to attach
            tracing::warn!(
                "Target.createTarget returned targetId without sessionId, falling back to HTTP"
            );
        } else {
            tracing::warn!("Target.createTarget failed, trying HTTP /json/new");
        }

        // Fallback: HTTP PUT /json/new (works on Dia, handles missing sessionId)
        Self::create_via_http(conn, ws_url, url).await
    }

    /// Create a target via the HTTP `/json/new` endpoint.
    /// This endpoint is available on all Chromium-based browsers.
    async fn create_via_http(conn: &mut Connection, ws_url: &str, url: &str) -> Result<Self> {
        // Parse port from ws_url: ws://127.0.0.1:PORT/devtools/browser/...
        let parsed = Url::parse(ws_url).map_err(|_| CdpError::CommandFailed {
            method: "create_via_http".into(),
            msg: "invalid ws_url".into(),
        })?;
        let port = parsed.port().ok_or_else(|| CdpError::CommandFailed {
            method: "create_via_http".into(),
            msg: "no port in ws_url".into(),
        })?;
        let host = parsed.host_str().unwrap_or("127.0.0.1");

        let http_url = format!("http://{}:{}/json/new?{}", host, port, url);

        tracing::debug!("HTTP create target: {}", http_url);

        let client = reqwest::Client::new();
        let resp = client
            .put(&http_url)
            .send()
            .await
            .map_err(|e| CdpError::CommandFailed {
                method: "create_via_http".into(),
                msg: format!("HTTP request failed: {e}"),
            })?;

        let target_info: Value = resp.json().await.map_err(|e| CdpError::CommandFailed {
            method: "create_via_http".into(),
            msg: format!("HTTP response parse failed: {e}"),
        })?;

        let target_id = target_info
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CdpError::CommandFailed {
                method: "create_via_http".into(),
                msg: "no id in HTTP response".into(),
            })?
            .to_string();

        tracing::debug!("Created target via HTTP: target={}", target_id);

        // Attach to the target
        let attach_result = conn
            .call(
                "Target.attachToTarget",
                json!({
                    "targetId": target_id,
                    "flatten": true,
                }),
            )
            .await?;

        let session_id = attach_result
            .get("sessionId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CdpError::CommandFailed {
                method: "Target.attachToTarget".into(),
                msg: "no sessionId in response".into(),
            })?
            .to_string();

        tracing::info!(
            "Attached to created target: {} (session={})",
            target_id,
            session_id
        );

        Ok(Tab {
            session_id,
            target_id,
        })
    }

    /// Navigate the tab to a URL. Waits for the page to reach a stable state.
    pub async fn navigate(&self, conn: &mut Connection, url: &str) -> Result<()> {
        tracing::info!("Navigating to: {}", url);

        // Use Target.attachToTarget first if we're using a fresh target
        let result = conn
            .call_with_session(&self.session_id, "Page.enable", json!({}))
            .await?;
        tracing::debug!("Page.enable: {:?}", result);

        // Navigate
        let _result = conn
            .call_with_session(
                &self.session_id,
                "Page.navigate",
                json!({
                    "url": url,
                }),
            )
            .await?;

        // Wait for load event or timeout
        // We just wait for the page's readyState
        self.wait_for_page_load(conn, 10000).await
    }

    /// Wait for the page to reach `loading_done` (complete) state.
    async fn wait_for_page_load(&self, conn: &mut Connection, timeout_ms: u64) -> Result<()> {
        let start = std::time::Instant::now();
        let timeout = Duration::from_millis(timeout_ms);

        loop {
            if start.elapsed() > timeout {
                tracing::warn!("Page load timed out after {}ms", timeout_ms);
                return Ok(()); // Don't fail on timeout — content may still be partial
            }

            let result = conn
                .call_with_session(
                    &self.session_id,
                    "Runtime.evaluate",
                    json!({
                        "expression": "document.readyState",
                        "returnByValue": true,
                    }),
                )
                .await?;

            let ready_state = result["result"]["value"].as_str().unwrap_or("unknown");

            if ready_state == "complete" {
                tracing::debug!("Page loaded (readyState=complete)");
                return Ok(());
            }

            // Also check if we have at least some content
            let has_content = conn.call_with_session(&self.session_id, "Runtime.evaluate", json!({
                "expression": "document.body ? document.body.innerText.length > 100 : false",
                "returnByValue": true,
            })).await?;

            if has_content["result"]["value"].as_bool().unwrap_or(false) {
                tracing::debug!("Page has sufficient content (readyState={})", ready_state);
                return Ok(());
            }

            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// Evaluate JavaScript in the tab context and return the result.
    pub async fn evaluate(&self, conn: &mut Connection, js: &str) -> Result<Value> {
        let result = conn
            .call_with_session(
                &self.session_id,
                "Runtime.evaluate",
                json!({
                    "expression": js,
                    "returnByValue": true,
                }),
            )
            .await?;

        Ok(result)
    }

    /// Extract the page content. Returns innerText of body.
    pub async fn extract_text(&self, conn: &mut Connection) -> Result<String> {
        let result = self
            .evaluate(conn, "document.body?.innerText || ''")
            .await?;
        let text = result["result"]["value"].as_str().unwrap_or("").to_string();
        Ok(text)
    }

    /// Extract the page title.
    pub async fn extract_title(&self, conn: &mut Connection) -> Result<String> {
        let result = self.evaluate(conn, "document.title || ''").await?;
        let title = result["result"]["value"].as_str().unwrap_or("").to_string();
        Ok(title)
    }

    /// Extract HTML content
    pub async fn extract_html(&self, conn: &mut Connection) -> Result<String> {
        let result = self
            .evaluate(conn, "document.documentElement?.outerHTML || ''")
            .await?;
        let html = result["result"]["value"].as_str().unwrap_or("").to_string();
        Ok(html)
    }

    /// Execute multiple JS snippets in sequence.
    pub async fn evaluate_all(
        &self,
        conn: &mut Connection,
        scripts: &[&str],
    ) -> Result<Vec<Value>> {
        let mut results = Vec::with_capacity(scripts.len());
        for script in scripts {
            let result = self.evaluate(conn, script).await?;
            results.push(result);
        }
        Ok(results)
    }

    /// Close this tab. Uses `window.close()` first (clean), then `Target.closeTarget` as fallback.
    /// Errors are logged but not propagated — non-blocking fire-and-forget.
    pub async fn close(self, conn: &mut Connection) {
        // Try window.close() first (cleaner)
        let _ = conn
            .call_with_session(
                &self.session_id,
                "Runtime.evaluate",
                json!({
                    "expression": "window.close()",
                }),
            )
            .await;

        // Wait for clean shutdown (200ms gives browser more time)
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Force close via CDP (ignore errors)
        let _ = conn
            .call(
                "Target.closeTarget",
                json!({
                    "targetId": self.target_id,
                }),
            )
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tab_struct_creation() {
        let tab = Tab {
            session_id: "test-session".into(),
            target_id: "test-target".into(),
        };
        assert_eq!(tab.session_id, "test-session");
        assert_eq!(tab.target_id, "test-target");
    }
}
