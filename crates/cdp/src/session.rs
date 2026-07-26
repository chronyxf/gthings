use crate::connection::{CdpEvent, Connection};
use crate::error::{CdpError, Result};
use crate::tab::Tab;
use gthings_common::domain_reputation::QualityFlag;
use serde_json::Value;
use std::time::Duration;
use tokio::sync::broadcast;

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

    /// Runs a compact JS snippet in the page to detect quality issues before extraction.
    /// The JS snippet checks for Cloudflare/Turnstile bot walls, reCAPTCHA/hCaptcha, and paywall text markers.
    /// Keep JS logic in sync with `Session::parse_signal_flags` and `gthings_extraction::quality::detection`.
    pub async fn check_page_signals(&self, tab: &Tab) -> Result<Vec<QualityFlag>> {
        let js = r#"
            (() => {
                const flags = [];
                if (document.querySelector('#cf-challenge, .cf-turnstile, [class*="challenge"], [id*="challenge"]'))
                    flags.push("BotWall");
                if (document.title.toLowerCase().includes("just a moment"))
                    flags.push("BotWall");
                if (document.querySelector('iframe[src*="recaptcha"], iframe[src*="hcaptcha"], .h-captcha, .g-recaptcha'))
                    flags.push("Captcha");
                const text = (document.body?.innerText || '').slice(0, 2000).toLowerCase();
                if (/subscribe to continue|sign in to read|you have reached your free article limit|subscribe to read|log in to read this/i.test(text))
                    flags.push("Paywall");
                return flags;
            })()
        "#;

        let result = tab.evaluate(self, js).await?;
        Ok(Self::parse_signal_flags(&result))
    }

    /// Wait for a CDP event matching method + predicate.
    ///
    /// Warning: Creates a new event subscription. Subscribe BEFORE the action that
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

    /// Parse a `Runtime.evaluate` result value into quality flags.
    ///
    /// Public and crate-visible for testing. The JS snippet returns an array
    /// of strings like `["BotWall", "Captcha"]`.
    pub(crate) fn parse_signal_flags(value: &Value) -> Vec<QualityFlag> {
        match value
            .get("result")
            .and_then(|r| r.get("value"))
            .and_then(|v| v.as_array())
        {
            Some(arr) => arr
                .iter()
                .filter_map(|v| {
                    v.as_str().and_then(|s| match s {
                        "BotWall" => Some(QualityFlag::BotWall),
                        "Captcha" => Some(QualityFlag::Captcha),
                        "Paywall" => Some(QualityFlag::Paywall),
                        "EmptyShell" => Some(QualityFlag::EmptyShell),
                        "Garbled" => Some(QualityFlag::Garbled),
                        "ThinContent" => Some(QualityFlag::ThinContent),
                        "Truncated" => Some(QualityFlag::Truncated),
                        _ => None,
                    })
                })
                .collect(),
            None => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_parse_signal_flags_empty() {
        let val = json!({"result": {"type": "object", "value": []}});
        let flags = Session::parse_signal_flags(&val);
        assert!(flags.is_empty());
    }

    #[test]
    fn test_parse_signal_flags_botwall() {
        let val = json!({"result": {"type": "object", "value": ["BotWall"]}});
        let flags = Session::parse_signal_flags(&val);
        assert_eq!(flags, vec![QualityFlag::BotWall]);
    }

    #[test]
    fn test_parse_signal_flags_multiple() {
        let val = json!({"result": {"type": "object", "value": ["BotWall", "Captcha"]}});
        let flags = Session::parse_signal_flags(&val);
        assert_eq!(flags, vec![QualityFlag::BotWall, QualityFlag::Captcha]);
    }

    #[test]
    fn test_parse_signal_flags_paywall() {
        let val = json!({"result": {"type": "object", "value": ["Paywall"]}});
        let flags = Session::parse_signal_flags(&val);
        assert_eq!(flags, vec![QualityFlag::Paywall]);
    }

    #[test]
    fn test_parse_signal_flags_unknown_ignored() {
        let val =
            json!({"result": {"type": "object", "value": ["BotWall", "UnknownFlag", "Captcha"]}});
        let flags = Session::parse_signal_flags(&val);
        assert_eq!(flags, vec![QualityFlag::BotWall, QualityFlag::Captcha]);
    }

    #[test]
    fn test_parse_signal_flags_missing_result() {
        let val = json!({});
        let flags = Session::parse_signal_flags(&val);
        assert!(flags.is_empty());
    }

    #[test]
    fn test_parse_signal_flags_non_array_value() {
        let val = json!({"result": {"type": "string", "value": "not_an_array"}});
        let flags = Session::parse_signal_flags(&val);
        assert!(flags.is_empty());
    }
}
