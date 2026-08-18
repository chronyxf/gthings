//! `POST /job` — enqueue a search job and stream its progress as SSE.
//!
//! The handler validates the wire shape and per-op args (400
//! `invalid-input`), refuses work when the queue is full (429
//! `rate_limited`) or the aggregate paid-API quota is exhausted (429
//! `quota_exceeded`), then registers the job's SSE channel with the
//! [`JobRegistry`] and enqueues it. The response is an SSE stream
//! (`job_started` → `result*` / `engine_event*` → `done` | `error`) built from
//! [`crate::sse::sse_stream`]; the terminal `done` event carries the complete
//! result envelope (query echo + results + the client-supplied trace id).

use std::convert::Infallible;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures::stream::{Stream, StreamExt};
use gthings_common::envelope::{Envelope, ErrorBody};
use gthings_common::taxonomy::ErrorCode;
use gthings_common::telemetry::StderrEvent;
use serde::Serialize;
use serde_json::json;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use crate::api::{AppState, pacing};
use crate::core::queue::EnqueueError;
use crate::core::workers::JobRegistry;
use crate::jobs::args::{EngineArg, JobArgs, invalid_input as invalid_input_body};
use crate::jobs::{Job, QueuedJob};
use crate::sse::{SseEvent, sse_stream_from};

/// Retry hint returned on a queue-full 429 (milliseconds).
pub(crate) const RETRY_AFTER_MS: u64 = 1000;
/// Capacity of one job's SSE event channel.
pub(crate) const SSE_CHANNEL_CAPACITY: usize = 64;
/// Wire code for a 503 while the daemon is draining.
pub(crate) const WIRE_CODE_UNAVAILABLE: &str = "unavailable";
/// Wire code for a 409 when the job's `trace_id` is already registered.
pub(crate) const WIRE_CODE_DUPLICATE_TRACE_ID: &str = "duplicate_trace_id";

/// Handle `POST /job`: validate, enqueue, and stream the job's progress.
pub(crate) async fn submit(State(state): State<AppState>, body: String) -> Response {
    // 1. Parse the wire shape; reject malformed payloads up front.
    let mut job: Job = match serde_json::from_str(&body) {
        Ok(job) => job,
        Err(error) => {
            return invalid_input(format!("malformed job payload: {error}"));
        }
    };

    // 2. Validate the per-op arguments (search, extract, ax, pdf-url, pdf-file).
    let args = match JobArgs::parse(job.op, &job.args) {
        Ok(args) => args,
        Err(body) => return error_response(StatusCode::BAD_REQUEST, body),
    };

    // 3. Refuse new work once draining has begun.
    if !state.shutdown.is_accepting() {
        return unavailable_response();
    }

    // 4. Refuse jobs that would immediately hit an exhausted aggregate quota.
    let (quota_exceeded, spend, limit) = quota_snapshot();
    if args.engine == EngineArg::Auto && quota_exceeded {
        return quota_exceeded_response(spend, limit);
    }

    // 5. Register the SSE channel under a stable trace id *before* enqueueing
    //    so the worker can publish progress even if it starts instantly. A
    //    duplicate trace id would silently overwrite the original job's SSE
    //    sender and cross-wire its stream, so it is rejected with 409.
    let trace_id = job
        .trace_id
        .get_or_insert_with(|| uuid::Uuid::new_v4().to_string())
        .clone();
    let (tx, rx) = mpsc::channel(SSE_CHANNEL_CAPACITY);
    if !state.registry.register(trace_id.clone(), tx).await {
        return duplicate_trace_id_response(&trace_id);
    }

    // Emit a structured start event so the Go side observes the job submission.
    let _ = StderrEvent::new(
        "info",
        trace_id.clone(),
        json!({ "op": job.op, "event": "job-submitted" }),
    )
    .emit();

    // 6. Enqueue the pre-validated `QueuedJob`; undo the registration when the
    //    queue refuses the job.
    let queued = QueuedJob {
        op: job.op,
        args,
        timeout_ms: job.timeout_ms,
        trace_id: Some(trace_id.clone()),
    };
    if let Err(error) = state.queue.enqueue(queued).await {
        state.registry.unregister(&trace_id).await;
        return enqueue_error_response(error);
    }

    crate::api::metrics::inc_jobs_submitted();

    sse_response(state.registry, trace_id, rx)
}

/// Snapshot the aggregate paid-API quota once: whether it is exhausted plus
/// the current spend/limit for a quota-exceeded response. Only `Auto` routing
/// may select paid engines — the free-engine pins (`Brave`, `Bing`, `Google`)
/// are never quota-gated — so the caller still checks `engine == Auto`.
fn quota_snapshot() -> (bool, u64, u64) {
    let pacing = pacing();
    (
        pacing.quota_exceeded(),
        pacing.quota_spend(),
        pacing.quota_limit(),
    )
}

/// Map an enqueue failure onto its HTTP response.
fn enqueue_error_response(error: EnqueueError) -> Response {
    match error {
        EnqueueError::Full => rate_limited_response(),
        EnqueueError::Closed => unavailable_response(),
    }
}

/// Build the SSE response for a registered job.
///
/// Frames come from [`sse_stream_from`]; a [`CleanupStream`] wrapper
/// unregisters the job's channel from the [`JobRegistry`] once the stream ends
/// — whether after the terminal `done`/`error` frame or on client disconnect —
/// so the receiver closes and the stream terminates cleanly.
fn sse_response(
    registry: Arc<JobRegistry>,
    trace_id: String,
    rx: mpsc::Receiver<SseEvent>,
) -> Response {
    let events = CleanupStream {
        inner: ReceiverStream::new(rx),
        registry,
        trace_id,
        scheduled: false,
    };
    let frames = sse_stream_from(events).map(|frame| Ok::<_, Infallible>(Bytes::from(frame)));
    let body = Body::from_stream(frames);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(body)
        .expect("static SSE response headers are valid")
}

/// Wraps an SSE event stream and unregisters the job's channel once the
/// terminal `done` event passes through, the stream ends, or the wrapper is
/// dropped (client disconnect).
struct CleanupStream<S> {
    inner: S,
    registry: Arc<JobRegistry>,
    trace_id: String,
    scheduled: bool,
}

impl<S> CleanupStream<S> {
    /// Spawn the registry unregistration exactly once. Dropping the registry's
    /// sender closes the underlying channel, ending the SSE stream.
    fn schedule(&mut self) {
        if self.scheduled {
            return;
        }
        self.scheduled = true;
        let registry = Arc::clone(&self.registry);
        let trace_id = self.trace_id.clone();
        tokio::spawn(async move { registry.unregister(&trace_id).await });
    }
}

impl<S> Stream for CleanupStream<S>
where
    S: Stream<Item = SseEvent> + Unpin,
{
    type Item = SseEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let ready = self.inner.poll_next_unpin(cx);
        match &ready {
            Poll::Ready(None) => self.schedule(),
            Poll::Ready(Some(event)) if event.is_terminal() => self.schedule(),
            Poll::Ready(Some(_)) | Poll::Pending => {}
        }
        ready
    }
}

impl<S> Drop for CleanupStream<S> {
    fn drop(&mut self) {
        self.schedule();
    }
}

/// Build a 400 `invalid-input` envelope response.
fn invalid_input(detail: impl Into<String>) -> Response {
    error_response(StatusCode::BAD_REQUEST, invalid_input_body(detail))
}

/// Serialize an [`ErrorBody`] as a JSON envelope at `status`.
fn error_response(status: StatusCode, body: ErrorBody) -> Response {
    json_response(status, Envelope::<serde_json::Value>::error(body))
}

/// Build a JSON response at `status`.
fn json_response<T: Serialize>(status: StatusCode, value: T) -> Response {
    (status, axum::Json(value)).into_response()
}

/// 429 with a retry hint when the bounded queue is full.
fn rate_limited_response() -> Response {
    json_response(
        StatusCode::TOO_MANY_REQUESTS,
        json!({ "code": ErrorCode::RateLimited.as_str(), "retry_after_ms": RETRY_AFTER_MS }),
    )
}

/// 429 when the aggregate paid-API quota is exhausted.
fn quota_exceeded_response(spend: u64, limit: u64) -> Response {
    crate::api::metrics::inc_jobs_429_quota();
    json_response(
        StatusCode::TOO_MANY_REQUESTS,
        json!({
            "code": ErrorCode::QuotaExceeded.as_str(),
            "spend": spend,
            "limit": limit,
        }),
    )
}

/// 503 while the daemon is draining (queue closed / accepting flag off).
fn unavailable_response() -> Response {
    json_response(
        StatusCode::SERVICE_UNAVAILABLE,
        json!({ "code": WIRE_CODE_UNAVAILABLE, "detail": "daemon is shutting down" }),
    )
}

/// 409 when the job's `trace_id` is already registered to a live SSE stream.
///
/// The registry is keyed by trace id, so accepting a duplicate would silently
/// overwrite the original job's sender and cross-wire its SSE stream.
fn duplicate_trace_id_response(trace_id: &str) -> Response {
    crate::api::metrics::inc_jobs_409_duplicate();
    json_response(
        StatusCode::CONFLICT,
        json!({
            "code": WIRE_CODE_DUPLICATE_TRACE_ID,
            "detail": "trace_id already registered to a live stream",
            "trace_id": trace_id,
        }),
    )
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;
    use axum::http::StatusCode;

    use super::{
        EnqueueError, RETRY_AFTER_MS, duplicate_trace_id_response, enqueue_error_response,
        invalid_input,
    };
    use crate::sse::SseEvent;
    use axum::response::Response;
    use gthings_common::taxonomy::ErrorCode;

    async fn response_json(response: Response) -> serde_json::Value {
        let (_, body) = response.into_parts();
        let bytes = to_bytes(body, 4096).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn full_queue_maps_to_429_rate_limited() {
        let response = enqueue_error_response(EnqueueError::Full);
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        let value = response_json(response).await;
        assert_eq!(value["code"], "rate-limited");
        assert_eq!(value["retry_after_ms"], RETRY_AFTER_MS);
    }

    #[tokio::test]
    async fn closed_queue_maps_to_503_unavailable() {
        let response = enqueue_error_response(EnqueueError::Closed);
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let value = response_json(response).await;
        assert_eq!(value["code"], "unavailable");
    }

    #[test]
    fn invalid_input_answers_400_bad_request() {
        let response = invalid_input("bad payload".to_string());
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn only_done_is_terminal() {
        assert!(
            SseEvent::Done {
                query: None,
                results: vec![],
                trace_id: None,
                count: 0,
                engine: None,
                duration_ms: 0,
                sla_ms: 0,
                queries: vec![],
                content: None,
            }
            .is_terminal()
        );
        assert!(
            !SseEvent::Error {
                code: ErrorCode::RateLimited,
                message: "429".into(),
                engine: None,
                retry_after_ms: None,
            }
            .is_terminal()
        );
        assert!(!SseEvent::JobStarted { trace_id: None }.is_terminal());
    }

    #[tokio::test]
    async fn duplicate_trace_id_maps_to_409_conflict() {
        let response = duplicate_trace_id_response("abc-123");
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let value = response_json(response).await;
        assert_eq!(value["code"], "duplicate_trace_id");
        assert_eq!(
            value["detail"],
            "trace_id already registered to a live stream"
        );
        assert_eq!(value["trace_id"], "abc-123");
    }
}
