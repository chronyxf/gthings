//! HTTP API layer for the serve daemon.
//!
//! [`AppState`] is the process-wide state shared by every handler: the bounded
//! job queue, the trace_id → SSE-sender registry, the browser metadata served
//! by `/healthz`, and the cooperative shutdown flag. [`router`] assembles the
//! axum [`Router`] serving `GET /healthz`, `POST /job`, and `GET /metrics`,
//! bound by [`crate::run`] to `config.serve_bind` (default `127.0.0.1:9080`).

pub(crate) mod healthz;
pub(crate) mod job;
pub(crate) mod metrics;

use std::sync::Arc;

use axum::Router;
use axum::routing::{get, post};
use gthings_cdp::DetectedBrowser;
use gthings_search::engine::pacing::global_pacing;

use crate::core::queue::JobQueue;
use crate::core::shutdown::Shutdown;
use crate::core::workers::JobRegistry;

/// Default CDP debugging port when `GTHINGS_CDP_PORT` is unset (matches the
/// CLI's `--cdp-port` default).
pub(crate) const DEFAULT_CDP_PORT: u16 = 9222;

/// Lock the process-wide pacing store, recovering from a poisoned mutex.
///
/// Shared by `/healthz` (pacing snapshot) and `POST /job` (quota snapshot) so
/// the poison-handling policy lives in exactly one place.
pub(crate) fn pacing() -> std::sync::MutexGuard<'static, gthings_search::engine::pacing::PacingStore>
{
    global_pacing().lock().unwrap_or_else(|e| e.into_inner())
}

/// Process-wide state shared by every HTTP handler.
#[derive(Debug, Clone)]
pub(crate) struct AppState {
    /// Bounded job queue; `POST /job` enqueues here.
    pub queue: Arc<JobQueue>,
    /// trace_id → SSE-sender registry bridging jobs to their streams.
    pub registry: Arc<JobRegistry>,
    /// Browser metadata discovered at startup (served by `/healthz`).
    pub browser: Option<DetectedBrowser>,
    /// Outcome of the startup browser detection, e.g. `"connected"`,
    /// `"detected-no-session"`, or `"not-detected"`.
    pub browser_status: Option<String>,
    /// Human-readable reason for a non-`"connected"` browser status.
    pub browser_reason: Option<String>,
    /// Cooperative accept/shutdown flag (503 after a termination signal).
    pub shutdown: Shutdown,
}

/// Resolve the CDP debugging port: `GTHINGS_CDP_PORT`, else
/// [`DEFAULT_CDP_PORT`].
#[must_use]
pub(crate) fn cdp_port() -> u16 {
    std::env::var("GTHINGS_CDP_PORT")
        .ok()
        .and_then(|port| port.parse().ok())
        .unwrap_or(DEFAULT_CDP_PORT)
}

/// Build the axum [`Router`] serving the daemon's HTTP endpoints.
pub(crate) fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz::healthz))
        .route("/job", post(job::submit))
        .route("/metrics", get(metrics::metrics))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state)
}
