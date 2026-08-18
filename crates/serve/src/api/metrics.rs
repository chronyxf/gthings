//! Prometheus-style `/metrics` endpoint.
//!
//! Exposes process-wide counters (jobs submitted, quota/duplicate rejections)
//! alongside the live queue-depth gauge read from [`AppState`] in the text
//! exposition format (`text/plain; version=0.0.4`).

use std::sync::atomic::{AtomicU64, Ordering};

use axum::extract::State;
use axum::http::header;
use axum::response::IntoResponse;

use crate::api::AppState;

/// Total `POST /job` requests accepted and enqueued.
static JOBS_SUBMITTED: AtomicU64 = AtomicU64::new(0);
/// `POST /job` requests rejected with a 429 `quota-exceeded`.
static JOBS_429_QUOTA: AtomicU64 = AtomicU64::new(0);
/// `POST /job` requests rejected with a 409 `duplicate_trace_id`.
static JOBS_409_DUPLICATE: AtomicU64 = AtomicU64::new(0);

/// Record a successfully enqueued job.
pub(crate) fn inc_jobs_submitted() {
    JOBS_SUBMITTED.fetch_add(1, Ordering::Relaxed);
}

/// Record a 429 `quota-exceeded` rejection.
pub(crate) fn inc_jobs_429_quota() {
    JOBS_429_QUOTA.fetch_add(1, Ordering::Relaxed);
}

/// Record a 409 `duplicate_trace_id` rejection.
pub(crate) fn inc_jobs_409_duplicate() {
    JOBS_409_DUPLICATE.fetch_add(1, Ordering::Relaxed);
}

/// Render the process-wide job counters and the live queue gauge as
/// Prometheus text exposition (`text/plain; version=0.0.4`).
pub(crate) async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    let queue_depth = state.queue.depth();
    let body = format!(
        "# HELP gthings_jobs_submitted Total search jobs submitted.\n\
         # TYPE gthings_jobs_submitted counter\n\
         gthings_jobs_submitted {jobs_submitted}\n\
         # HELP gthings_jobs_429_quota Jobs rejected with 429 quota-exceeded.\n\
         # TYPE gthings_jobs_429_quota counter\n\
         gthings_jobs_429_quota {jobs_429_quota}\n\
         # HELP gthings_jobs_409_duplicate Jobs rejected with 409 duplicate trace id.\n\
         # TYPE gthings_jobs_409_duplicate counter\n\
         gthings_jobs_409_duplicate {jobs_409_duplicate}\n\
         # HELP gthings_queue_depth Jobs currently waiting in the bounded queue.\n\
         # TYPE gthings_queue_depth gauge\n\
         gthings_queue_depth {queue_depth}\n",
        jobs_submitted = JOBS_SUBMITTED.load(Ordering::Relaxed),
        jobs_429_quota = JOBS_429_QUOTA.load(Ordering::Relaxed),
        jobs_409_duplicate = JOBS_409_DUPLICATE.load(Ordering::Relaxed),
        queue_depth = queue_depth,
    );
    ([(header::CONTENT_TYPE, "text/plain; version=0.0.4")], body).into_response()
}
