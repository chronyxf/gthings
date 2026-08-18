use super::user_agent::user_agent;
use crate::connection::CdpEvent;
use crate::error::{CdpError, Result};
use crate::session::Session;
use crate::tab::Tab;
use std::time::Duration;
use tokio::sync::broadcast;

/// Timeout for the `Page.lifecycleEvent` (networkIdle) wait.
const NAV_TIMEOUT: Duration = Duration::from_secs(10);
/// Number of `document.readyState` polls in the fallback path.
const READYSTATE_POLLS: u32 = 10;
/// Interval between `document.readyState` polls.
const READYSTATE_INTERVAL: Duration = Duration::from_millis(500);

/// Outcome of a single [`recv_event`] call, mapping the broadcast channel's
/// `Closed`/`Lagged` error variants into a shared shape.
pub(crate) enum RecvOutcome {
    Event(CdpEvent),
    Closed,
    Lagged(()),
}

/// Receive the next event from a broadcast receiver, mapping `Closed` and
/// `Lagged` into [`RecvOutcome`]. Shared by the navigation and dialog wait
/// loops.
pub(crate) async fn recv_event(rx: &mut broadcast::Receiver<CdpEvent>) -> RecvOutcome {
    match rx.recv().await {
        Ok(event) => RecvOutcome::Event(event),
        Err(broadcast::error::RecvError::Closed) => RecvOutcome::Closed,
        Err(broadcast::error::RecvError::Lagged(_)) => {
            tracing::warn!("Event receiver lagged");
            RecvOutcome::Lagged(())
        }
    }
}

/// Shared wait loop body: receives events from the broadcast channel,
/// filters by method + predicate, and returns the matching event or an error.
pub(crate) async fn wait_impl(
    rx: &mut broadcast::Receiver<CdpEvent>,
    method: &str,
    predicate: impl Fn(&CdpEvent) -> bool + Send,
) -> Result<CdpEvent> {
    loop {
        match recv_event(rx).await {
            RecvOutcome::Event(event) if event.method.as_str() == method && predicate(&event) => {
                return Ok(event);
            }
            RecvOutcome::Event(_) => continue,
            RecvOutcome::Closed => {
                return Err(CdpError::ConnectionFailed {
                    detail: "event channel closed while waiting".into(),
                });
            }
            RecvOutcome::Lagged(()) => continue,
        }
    }
}

/// Wait for a specific CDP event on a pre-subscribed receiver, mapping a
/// timeout into the caller's error variant.
/// Subscribe BEFORE the triggering action to avoid missing the event.
pub(crate) async fn wait_for_event(
    rx: &mut broadcast::Receiver<CdpEvent>,
    method: &str,
    predicate: impl Fn(&CdpEvent) -> bool + Send,
    timeout: Duration,
    map_err: impl FnOnce() -> CdpError,
) -> Result<CdpEvent> {
    tokio::time::timeout(timeout, wait_impl(rx, method, predicate))
        .await
        .map_err(|_| map_err())?
}

impl Session {
    /// Navigate to URL and wait for networkIdle lifecycle event
    pub async fn navigate(&self, tab: &Tab, url: &str) -> Result<()> {
        // B1 SSRF gate: only http/https/about:blank may reach Chrome.
        crate::error::validate_scheme(url)?;
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
        let ua = user_agent();
        if let Err(e) = conn
            .call(
                "Network.setUserAgentOverride",
                serde_json::json!({
                    "userAgent": ua,
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
            NAV_TIMEOUT,
            || CdpError::NavigationTimeout {
                url: url.to_string(),
                timeout: NAV_TIMEOUT.as_secs(),
            },
        )
        .await;

        // Fallback: if lifecycle event timed out, poll document.readyState
        match result {
            Ok(_) => {}
            Err(CdpError::NavigationTimeout { .. }) => {
                tracing::warn!("Lifecycle event timeout, falling back to readyState polling");
                // Poll document.readyState up to READYSTATE_POLLS more times.
                for _ in 0..READYSTATE_POLLS {
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
                    tokio::time::sleep(READYSTATE_INTERVAL).await;
                }
                return Err(CdpError::NavigationTimeout {
                    url: url.to_string(),
                    timeout: (NAV_TIMEOUT + READYSTATE_POLLS * READYSTATE_INTERVAL).as_secs(),
                });
            }
            Err(e) => return Err(e),
        }

        Ok(())
    }
}
