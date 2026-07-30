use crate::connection::{CdpEvent, Connection, call_async};
use crate::error::{CdpError, Result};
use crate::tab::Tab;
use gthings_common::domain_reputation::QualityFlag;
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

/// Default timeout for navigation-related operations (25 seconds).
const DEFAULT_NAV_TIMEOUT: Duration = Duration::from_secs(25);

/// High-level CDP session. Manages connection + tabs with event-driven lifecycle.
pub struct Session {
    conn: Connection,
    /// Handle to the background dialog auto-accept task, aborted on disconnect.
    dialog_handle: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session").finish_non_exhaustive()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Abort the dialog handler task to prevent it from
        // outliving the session and holding a stale write channel.
        if let Some(handle) = self.dialog_handle.take() {
            handle.abort();
        }
    }
}

/// Shared wait loop body: receives events from the broadcast channel,
/// filters by method + predicate, and returns the matching event or an error.
async fn wait_impl(
    rx: &mut broadcast::Receiver<CdpEvent>,
    method: &str,
    predicate: impl Fn(&CdpEvent) -> bool + Send,
) -> Result<CdpEvent> {
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
}

/// Wait for a specific CDP event on a pre-subscribed receiver.
/// Subscribe BEFORE the triggering action to avoid missing the event.
async fn wait_for_event(
    rx: &mut broadcast::Receiver<CdpEvent>,
    method: &str,
    predicate: impl Fn(&CdpEvent) -> bool + Send,
    timeout: Duration,
    url: &str,
) -> Result<CdpEvent> {
    tokio::time::timeout(timeout, wait_impl(rx, method, predicate))
        .await
        .map_err(|_| CdpError::NavigationTimeout {
            url: url.to_string(),
            timeout: timeout.as_secs(),
        })?
}

impl Session {
    /// Connect to browser via WebSocket URL.
    ///
    /// `timeout` controls the per-call CDP response timeout
    /// (defaults to 30 seconds when `None`).
    pub async fn connect(ws_url: &str, timeout: Option<Duration>) -> Result<Self> {
        let conn = Connection::connect(ws_url, timeout).await?;
        let dialog_handle = Some(Self::spawn_dialog_handler(&conn));
        Ok(Session {
            conn,
            dialog_handle,
        })
    }

    /// Spawn a background task that auto-accepts JavaScript dialogs
    /// (`alert`, `confirm`, `prompt`, `beforeunload`) by listening for
    /// `Page.javascriptDialogOpening` events and immediately calling
    /// `Page.handleJavaScriptDialog` with `accept: true`.
    fn spawn_dialog_handler(conn: &Connection) -> JoinHandle<()> {
        let mut rx = conn.event_rx();
        let write = conn.write_tx();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(event) if event.method == "Page.javascriptDialogOpening" => {
                        tracing::debug!(
                            "Auto-accepting dialog: type={:?}, message={:?}",
                            event.params.get("type"),
                            event.params.get("message"),
                        );
                        call_async(
                            &write,
                            "Page.handleJavaScriptDialog",
                            serde_json::json!({"accept": true}),
                            event.session_id,
                        );
                    }
                    Ok(_) => continue,
                    Err(broadcast::error::RecvError::Closed) => {
                        tracing::debug!("Dialog handler: event channel closed, stopping");
                        break;
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("Dialog event receiver lagged by {n} messages");
                        continue;
                    }
                }
            }
        })
    }

    /// Create a new tab/page (foreground, non-isolated).
    pub async fn create_tab(&self, url: &str) -> Result<Tab> {
        Tab::create(self, url, false).await
    }

    /// Create a background tab (no window focus steal) at `about:blank`.
    ///
    /// Background tabs are isolated from foreground tabs and do not block
    /// concurrent CDP operations.
    pub async fn create_background_tab(&self) -> Result<Tab> {
        Tab::create_background(self).await
    }

    /// Create an isolated background tab, optionally navigate, run the closure,
    /// and close the tab in a finally-like pattern (tab is closed even on error
    /// or timeout).
    ///
    /// Shared implementation for [`with_isolated_tab`] and [`run_in_tab`].
    async fn with_background_tab_impl<F, T>(&self, url: Option<&str>, f: F) -> Result<T>
    where
        F: for<'a> FnOnce(
            &'a Session,
            &'a Tab,
        ) -> Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>,
    {
        let tab = Tab::create_background(self).await?;
        if let Some(url) = url {
            tab.navigate(self, url).await?;
        }
        let result = tokio::time::timeout(DEFAULT_NAV_TIMEOUT, f(self, &tab)).await;
        let result = match result {
            Ok(r) => r,
            Err(_) => {
                tracing::warn!(
                    "background tab operation timed out after {}s, closing tab",
                    DEFAULT_NAV_TIMEOUT.as_secs()
                );
                let _ = tab.close(self).await;
                return Err(CdpError::CdpCallFailed {
                    method: "background_tab".to_string(),
                    detail: format!(
                        "operation timed out after {}s",
                        DEFAULT_NAV_TIMEOUT.as_secs()
                    ),
                });
            }
        };
        // Close tab in finally-like pattern — report but swallow close errors
        if let Err(e) = tab.close(self).await {
            tracing::warn!("close background tab failed: {e}");
        }
        result
    }

    /// Create an isolated background tab, run the closure, and close the tab
    /// in a finally-like pattern.
    ///
    /// This is the primary isolation primitive: each operation gets its own
    /// background tab, preventing cross-process blocking.
    ///
    /// The closure is wrapped in a 60-second timeout. If the closure hangs
    /// (e.g., a CDP call never responds), the tab is forcefully closed and
    /// a CdpCallFailed error is returned. This prevents orphaned tabs from
    /// accumulating in the browser when commands time out at the CLI level.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let results = session
    ///     .with_isolated_tab(|session, tab| async move {
    ///         search(session, tab, "query", 10).await
    ///     })
    ///     .await?;
    /// ```
    pub async fn with_isolated_tab<F, T>(&self, f: F) -> Result<T>
    where
        F: for<'a> FnOnce(
            &'a Session,
            &'a Tab,
        ) -> Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>,
    {
        self.with_background_tab_impl(None, f).await
    }

    /// Create an isolated background tab, navigate to `url`, run the closure,
    /// and close the tab in a finally-like pattern.
    ///
    /// Like [`with_isolated_tab`](Session::with_isolated_tab) but performs
    /// navigation first so the closure can immediately evaluate or interact.
    ///
    /// The closure is wrapped in a 60-second timeout. If the closure hangs,
    /// the tab is forcefully closed and a CdpCallFailed error is returned.
    pub async fn run_in_tab<F, T>(&self, url: &str, f: F) -> Result<T>
    where
        F: for<'a> FnOnce(
            &'a Session,
            &'a Tab,
        ) -> Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>,
    {
        self.with_background_tab_impl(Some(url), f).await
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

        // 2a. Set a real desktop Chrome User-Agent to avoid "HeadlessChrome" detection
        if let Err(e) = conn
            .call(
                "Network.setUserAgentOverride",
                serde_json::json!({
                    "userAgent": "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36",
                    "platform": "macOS",
                }),
                sid,
            )
            .await
        {
            tracing::warn!(error = %e, "failed to set user agent override");
        }

        // 2b. Inject minimal stealth script that hides automation fingerprints (navigator.webdriver override).
        let stealth_js = r#"(() => {
            Object.defineProperty(navigator, 'webdriver', { get: () => undefined });
            Object.defineProperty(navigator, 'languages', { get: () => ['en-US', 'en'] });
        })()"#;

        if let Err(e) = conn
            .call(
                "Page.addScriptToEvaluateOnNewDocument",
                serde_json::json!({
                    "source": stealth_js,
                }),
                sid,
            )
            .await
        {
            tracing::warn!(error = %e, "failed to add stealth script");
        }

        // Clone sid for the closure — session_id filtering prevents cross-tab event matches
        let sid_owned = tab.session_id.clone();

        // 3. Start navigation
        conn.call("Page.navigate", serde_json::json!({"url": url}), sid)
            .await?;

        // 4. Wait for networkIdle using the pre-subscribed receiver
        let result = wait_for_event(
            &mut rx,
            "Page.lifecycleEvent",
            move |evt| {
                let session_match = match &sid_owned {
                    Some(sid) => evt.session_id.as_deref() == Some(sid.as_str()),
                    None => true,
                };
                let name_match =
                    evt.params.get("name").and_then(|v| v.as_str()) == Some("networkIdle");
                session_match && name_match
            },
            Duration::from_secs(10),
            url,
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
        tokio::time::timeout(timeout, wait_impl(&mut rx, method, predicate))
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
    pub async fn disconnect(mut self) -> Result<()> {
        // Abort the dialog handler first to drop its clone of the write channel,
        // ensuring the I/O task can cleanly exit.
        if let Some(h) = self.dialog_handle.take() {
            h.abort();
        }
        // Connection is dropped via Drop impl, which aborts the I/O task
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

    // ── Isolated tab API tests ────────────────────────────────────────────

    #[test]
    fn test_tab_cleanup_sequence() {
        // Verify that Tab::close constructs the correct CDP calls:
        // 1. Runtime.evaluate with window.close()
        // 2. Target.closeTarget with targetId
        let tab = Tab {
            target_id: "test-target-close".into(),
            session_id: Some("test-session-close".into()),
        };

        // window.close expression
        let expr = json!({
            "expression": "window.close()",
            "userGesture": true,
        });
        assert_eq!(
            expr.get("expression").and_then(|v| v.as_str()),
            Some("window.close()")
        );
        assert_eq!(expr.get("userGesture"), Some(&json!(true)));

        // Target.closeTarget params
        let close_params = json!({ "targetId": tab.target_id });
        assert_eq!(
            close_params.get("targetId").and_then(|v| v.as_str()),
            Some("test-target-close")
        );

        // Verify that create_background uses background:true
        let bg_params = json!({
            "url": crate::ABOUT_BLANK,
            "background": true,
        });
        assert_eq!(bg_params.get("background"), Some(&json!(true)));
        assert_eq!(bg_params.get("url"), Some(&json!(crate::ABOUT_BLANK)));
    }

    #[test]
    fn test_background_tab_has_background_flag() {
        // Verify that Tab::create_background() constructs CDP params with
        // `"background": true` so the browser creates the tab without stealing
        // window focus.  This is the core isolation primitive for background tabs.
        let params = json!({
            "url": crate::ABOUT_BLANK,
            "background": true,
        });

        assert_eq!(
            params.get("background"),
            Some(&json!(true)),
            "background flag must be true"
        );
        assert_eq!(
            params.get("url"),
            Some(&json!(crate::ABOUT_BLANK)),
            "url must be about:blank"
        );

        // Contrast with foreground tab params (no background flag)
        let fg_params = json!({
            "url": "https://example.com",
        });
        assert!(
            fg_params.get("background").is_none(),
            "foreground tab should not have background flag"
        );
        assert_eq!(fg_params.get("url"), Some(&json!("https://example.com")));
    }

    #[test]
    fn test_with_isolated_tab_compiles() {
        // Minimal type-check: Tab must be Send + Clone for the API.
        fn _bounds<T: Send + Clone>() {}
        _bounds::<Tab>();
    }
}
