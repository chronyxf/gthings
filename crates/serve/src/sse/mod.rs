//! SSE event projection and frame encoding for the search-job stream.
//!
//! The worker consumes the [`gthings_search::SearchEvent`] mpsc stream and
//! projects it into [`SseEvent`]s that it forwards to the HTTP layer's SSE
//! channel. This module owns that projection ([`SearchEvent`] → [`SseEvent`]),
//! the raw frame encoder, and the [`sse_stream`] builder that interleaves an
//! idle heartbeat comment while a job is running.

pub(crate) mod events;

use std::future::Future;
use std::pin::Pin;
use std::task::Poll;
use std::time::Duration;

use futures::stream::{self, Stream, StreamExt};
use gthings_search::{EngineEventKind, SearchEngine, SearchEngineError};

pub(crate) use self::events::SseEvent;

/// Heartbeat cadence. SSE proxies and clients drop connections that stay idle
/// much longer than this, so a keepalive comment is emitted whenever the
/// underlying job stream has nothing to say for one full interval.
pub(crate) const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(25);

/// Keepalive comment frame, sent verbatim while a job is running. Comments
/// (lines starting with `:`) are ignored by SSE clients but reset proxy timeouts.
pub(crate) const HEARTBEAT_COMMENT: &str = ":heartbeat\n\n";

/// Project an engine lifecycle event into its SSE wire form.
#[must_use]
pub(crate) fn project_event(engine: SearchEngine, kind: EngineEventKind) -> SseEvent {
    SseEvent::EngineEvent { engine, kind }
}

/// Encode one event as a complete SSE frame: `event:<name>\ndata:<json>\n\n`.
#[must_use]
fn encode_frame(event: &SseEvent) -> String {
    let data = serde_json::to_string(event)
        .unwrap_or_else(|e| format!("{{\"error\":\"serialization failed: {e}\"}}"));
    format!("event:{}\ndata:{data}\n\n", event.event_name())
}

/// Build an SSE frame stream from a job's event receiver.
///
/// The channel carries already-projected [`SseEvent`]s — the worker emits the
/// terminal `done` frame with the complete result envelope (query + results +
/// trace_id) — so this layer only encodes frames and interleaves a keepalive
/// comment while the job is running. The stream ends as soon as the receiver
/// closes (right after the terminal `done` / `error` event).
/// Build an SSE frame stream from an arbitrary event stream, applying the
/// default heartbeat cadence/comment. Lets the HTTP layer wrap the event
/// stream in cleanup logic before encoding.
pub(crate) fn sse_stream_from(
    events: impl Stream<Item = SseEvent> + Unpin,
) -> impl Stream<Item = String> {
    sse_stream_with_heartbeat(events, HEARTBEAT_INTERVAL, HEARTBEAT_COMMENT)
}

/// [`sse_stream`] over an arbitrary event stream (testable; also lets the HTTP
/// layer wrap the stream in cleanup logic before encoding).
fn sse_stream_with_heartbeat<S>(
    events: S,
    interval: Duration,
    heartbeat_comment: &'static str,
) -> impl Stream<Item = String>
where
    S: Stream<Item = SseEvent> + Unpin,
{
    let mut events = events.map(|event| encode_frame(&event));
    let mut heartbeat = None::<Pin<Box<tokio::time::Sleep>>>;
    stream::poll_fn(move |cx| {
        // A pending real event always wins over the heartbeat.
        if let Poll::Ready(frame) = events.poll_next_unpin(cx) {
            return Poll::Ready(frame);
        }
        let sleep = heartbeat.get_or_insert_with(|| Box::pin(tokio::time::sleep(interval)));
        match sleep.as_mut().poll(cx) {
            Poll::Ready(()) => {
                heartbeat = None;
                Poll::Ready(Some(heartbeat_comment.to_string()))
            }
            Poll::Pending => Poll::Pending,
        }
    })
}

/// The engine implicated by a terminal error, when a single engine is at fault.
///
/// Shared by the SSE projection and the job workers (same crate) so the
/// engine attribution of a [`SearchEngineError`] is mapped in exactly one
/// place.
pub(crate) fn error_engine(error: &SearchEngineError) -> Option<SearchEngine> {
    match error {
        SearchEngineError::RateLimited { engine, .. }
        | SearchEngineError::Captcha { engine, .. }
        | SearchEngineError::QuotaExceeded { engine, .. }
        | SearchEngineError::Network { engine, .. }
        | SearchEngineError::Parse { engine, .. }
        | SearchEngineError::Unavailable { engine, .. } => Some(*engine),
        SearchEngineError::AllEnginesFailed(_) => None,
    }
}

/// The backend-supplied retry delay for a terminal error, when present.
///
/// Shared by the SSE projection and the job workers (same crate) so the
/// retry attribution of a [`SearchEngineError`] is mapped in exactly one
/// place.
pub(crate) fn error_retry_after_ms(error: &SearchEngineError) -> Option<u64> {
    match error {
        SearchEngineError::RateLimited { retry_after_ms, .. } => *retry_after_ms,
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        HEARTBEAT_COMMENT, SseEvent, encode_frame, project_event, sse_stream_with_heartbeat,
    };
    use crate::sse::events::test_result;
    use futures::stream::StreamExt;
    use gthings_search::{EngineEventKind, SearchEngine};
    use serde_json::json;
    use std::time::Duration;
    use tokio::sync::mpsc;

    #[test]
    fn projects_engine_events() {
        let cases = [
            (
                (SearchEngine::Bing, EngineEventKind::Captcha),
                json!({"event": "engine-event", "engine": "bing", "kind": "captcha"}),
            ),
            (
                (SearchEngine::Bing, EngineEventKind::RateLimited),
                json!({"event": "engine-event", "engine": "bing", "kind": "rate-limited"}),
            ),
        ];
        for ((engine, kind), expected) in cases {
            assert_eq!(
                serde_json::to_value(project_event(engine, kind)).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn encodes_events_as_sse_frames() {
        assert_eq!(
            encode_frame(&SseEvent::JobStarted { trace_id: None }),
            "event:job-started\ndata:{\"event\":\"job-started\"}\n\n"
        );
        assert_eq!(
            encode_frame(&SseEvent::Done {
                query: None,
                results: vec![],
                trace_id: None,
                count: 0,
                engine: None,
                duration_ms: 0,
                sla_ms: 0,
                queries: vec![],
                content: None,
            }),
            "event:done\ndata:{\"event\":\"done\",\"results\":[],\"count\":0,\"duration_ms\":0,\"sla_ms\":0,\"queries\":[]}\n\n"
        );
        // The done frame carries the complete envelope (query + results + trace id).
        let done = encode_frame(&SseEvent::Done {
            query: Some("rust".into()),
            results: vec![test_result()],
            trace_id: Some("t-1".into()),
            count: 1,
            engine: Some(SearchEngine::Brave),
            duration_ms: 42,
            sla_ms: 5000,
            queries: vec!["rust".into()],
            content: None,
        });
        assert!(done.starts_with("event:done\n"));
        assert!(done.contains("\"event\":\"done\""));
        assert!(done.contains("\"query\":\"rust\""));
        assert!(done.contains("\"trace_id\":\"t-1\""));
        assert!(done.contains("\"count\":1"));
        assert!(done.contains("\"engine\":\"brave\""));
        assert!(done.contains("\"duration_ms\":42"));
        assert!(done.contains("\"sla_ms\":5000"));
        assert!(done.contains("\"queries\":[\"rust\"]"));
        assert!(done.contains("\"results\":[{"));
        let frame = encode_frame(&SseEvent::EngineEvent {
            engine: SearchEngine::Google,
            kind: EngineEventKind::Cooldown,
        });
        assert!(frame.starts_with("event:engine-event\n"));
        assert!(frame.contains("\"engine\":\"google\""));
    }

    #[tokio::test]
    async fn stream_ends_after_terminal_event_without_heartbeat() {
        let (tx, rx) = mpsc::channel(8);
        let stream = sse_stream_with_heartbeat(
            tokio_stream::wrappers::ReceiverStream::new(rx),
            Duration::from_millis(20),
            ":hb\n\n",
        );
        tx.send(SseEvent::JobStarted { trace_id: None })
            .await
            .unwrap();
        tx.send(SseEvent::Result(test_result())).await.unwrap();
        tx.send(SseEvent::Done {
            query: Some("rust".into()),
            results: vec![test_result()],
            trace_id: Some("t-1".into()),
            count: 1,
            engine: Some(SearchEngine::Brave),
            duration_ms: 42,
            sla_ms: 5000,
            queries: vec!["rust".into()],
            content: None,
        })
        .await
        .unwrap();
        drop(tx);

        let frames = tokio::time::timeout(Duration::from_secs(1), stream.collect::<Vec<_>>())
            .await
            .expect("stream must terminate");
        assert_eq!(frames.len(), 3);
        assert!(frames[0].starts_with("event:job-started\n"));
        assert!(frames[1].starts_with("event:result\n"));
        assert!(frames[2].starts_with("event:done\n"));
        // The terminal done frame carries the full envelope.
        assert!(frames[2].contains("\"query\":\"rust\""));
        assert!(frames[2].contains("\"trace_id\":\"t-1\""));
        assert!(frames[2].contains("\"results\":[{"));
        assert!(!frames.iter().any(|f| f == ":hb\n\n"));
    }

    #[tokio::test]
    async fn emits_heartbeat_while_idle() {
        let (tx, rx) = mpsc::channel(8);
        let mut stream = sse_stream_with_heartbeat(
            tokio_stream::wrappers::ReceiverStream::new(rx),
            Duration::from_millis(20),
            HEARTBEAT_COMMENT,
        );
        let frame = tokio::time::timeout(Duration::from_secs(1), stream.next())
            .await
            .expect("heartbeat within timeout")
            .expect("stream not ended while sender alive");
        assert_eq!(frame, HEARTBEAT_COMMENT);
        drop(tx);
    }
}
