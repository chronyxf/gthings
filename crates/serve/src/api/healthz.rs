//! `GET /healthz` — daemon readiness and pacing visibility.
//!
//! Reports browser metadata, the queue depth, and a per-engine pacing
//! snapshot ([`EngineHealth`]) in hybrid priority order.

use axum::Json;
use axum::extract::State;
use serde::Serialize;

use crate::api::{AppState, pacing};

/// Per-engine pacing snapshot exposed by `/healthz`.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct EngineHealth {
    /// Engine wire id (e.g. `"brave"`).
    pub engine: String,
    /// Millis until the engine may be dispatched again (`0` = ready now).
    pub retry_after_ms: u64,
    /// Unix millis until which the engine is blocked, if in cooldown.
    pub cooldown_until_ms: Option<u64>,
    /// Unix millis of the engine's last dispatched query, if any.
    pub last_call_ms: Option<u64>,
}

/// The `/healthz` response shape.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Healthz {
    /// Browser name (e.g. `"Chrome"`), when a browser was found.
    pub browser: Option<String>,
    /// Browser version string, when a browser was found.
    pub version: Option<String>,
    /// CDP WebSocket URL of the warm session, when connected.
    pub ws_url: Option<String>,
    /// Outcome of the startup browser detection (e.g. `"connected"`,
    /// `"detected-no-session"`, `"not-detected"`).
    pub browser_status: Option<String>,
    /// Human-readable reason for a non-`"connected"` browser status.
    pub browser_reason: Option<String>,
    /// Jobs currently waiting in the bounded queue.
    pub queue_depth: usize,
    /// Pacing state for every engine, in hybrid priority order.
    pub engines: Vec<EngineHealth>,
}

/// Serve a readiness snapshot: browser metadata, queue depth, and the
/// per-engine pacing summary in hybrid priority order.
pub(crate) async fn healthz(State(state): State<AppState>) -> Json<Healthz> {
    Json(Healthz {
        browser: state.browser.as_ref().map(|b| b.browser.clone()),
        version: state.browser.as_ref().map(|b| b.version.clone()),
        ws_url: state.browser.as_ref().map(|b| b.ws_url.clone()),
        browser_status: state.browser_status.clone(),
        browser_reason: state.browser_reason.clone(),
        queue_depth: state.queue.depth(),
        engines: engines_health(),
    })
}

/// Snapshot every engine's pacing state, in hybrid priority order.
fn engines_health() -> Vec<EngineHealth> {
    let now_ms = gthings_common::util::time::unix_now_ms() as u64;
    pacing()
        .pacing_snapshot(now_ms)
        .into_iter()
        .map(|snapshot| EngineHealth {
            engine: snapshot.engine.as_str().to_string(),
            retry_after_ms: snapshot.retry_after_ms,
            cooldown_until_ms: snapshot.cooldown_until_ms,
            last_call_ms: snapshot.last_call_ms,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::Json;
    use axum::extract::State;
    use gthings_cdp::DetectedBrowser;

    use super::healthz;
    use crate::api::AppState;
    use crate::core::queue::JobQueue;
    use crate::core::shutdown::Shutdown;
    use crate::core::workers::JobRegistry;

    fn state_with(
        browser: Option<DetectedBrowser>,
        status: Option<&str>,
        reason: Option<&str>,
    ) -> AppState {
        AppState {
            queue: Arc::new(JobQueue::new(2, 1).0),
            registry: Arc::new(JobRegistry::new()),
            browser,
            browser_status: status.map(|s| s.to_string()),
            browser_reason: reason.map(|s| s.to_string()),
            shutdown: Shutdown::new(),
        }
    }

    #[tokio::test]
    async fn healthz_reports_browser_and_empty_queue() {
        let state = state_with(
            Some(DetectedBrowser {
                ws_url: "ws://localhost:9222/devtools/browser/abc".to_string(),
                browser: "Chrome".to_string(),
                version: "138.0.0.0".to_string(),
            }),
            Some("connected"),
            None,
        );
        let Json(body) = healthz(State(state)).await;
        assert_eq!(body.browser.as_deref(), Some("Chrome"));
        assert_eq!(body.version.as_deref(), Some("138.0.0.0"));
        assert_eq!(
            body.ws_url.as_deref(),
            Some("ws://localhost:9222/devtools/browser/abc")
        );
        assert_eq!(body.browser_status.as_deref(), Some("connected"));
        assert_eq!(body.browser_reason, None);
        assert_eq!(body.queue_depth, 0);
    }

    #[tokio::test]
    async fn healthz_omits_browser_when_not_detected() {
        let state = state_with(None, Some("not-detected"), Some("no browser on :9222"));
        let Json(body) = healthz(State(state)).await;
        assert_eq!(body.browser, None);
        assert_eq!(body.version, None);
        assert_eq!(body.ws_url, None);
        assert_eq!(body.browser_status.as_deref(), Some("not-detected"));
        assert_eq!(body.browser_reason.as_deref(), Some("no browser on :9222"));
        assert_eq!(body.queue_depth, 0);
    }
}
