//! SSE wire event types for the search-job event stream.
//!
//! [`SseEvent`] is the projection target of the internal
//! [`gthings_search::SearchEvent`] stream. It is internally tagged by the
//! `event` discriminator (serialized first, kebab-case) so consumers can route
//! on the event name without parsing the payload.

use serde::{Serialize, Serializer};

use gthings_common::taxonomy::ErrorCode;
use gthings_search::{EngineEventKind, SearchEngine, SearchResult};

#[cfg(test)]
use gthings_common::provenance::Provenance;
#[cfg(test)]
use gthings_search::EngineMode;

/// One SSE event emitted on the job stream.
///
/// Payloads mirror [`gthings_search::SearchEvent`] with engine and error data
/// normalized to wire-stable lowercase/kebab identifiers.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub(crate) enum SseEvent {
    /// The search job has started; the first event of every stream.
    JobStarted {
        /// The job's trace id, echoed verbatim (the client-supplied value
        /// when one was given, never a replacement id).
        #[serde(skip_serializing_if = "Option::is_none")]
        trace_id: Option<String>,
    },
    /// One mapped search result, emitted as it arrives.
    Result(SearchResult),
    /// An engine lifecycle notice (fallback / rate-limit / captcha / cooldown).
    EngineEvent {
        /// Wire id of the engine the event concerns (e.g. `"brave"`).
        #[serde(serialize_with = "serialize_engine")]
        engine: SearchEngine,
        /// What happened to that engine (kebab-case).
        #[serde(serialize_with = "serialize_engine_kind")]
        kind: EngineEventKind,
    },
    /// The search completed successfully; final event of a successful stream.
    ///
    /// Carries the complete result envelope — the query echo, every result
    /// collected for the job, and the job's trace id — so a client can
    /// assemble the full result set from this one terminal event without
    /// tracking the individual `result` frames.
    Done {
        /// The single-query echo for `simple` search jobs; absent for
        /// multi-query (`parallel`/`harvest`) and non-search ops, mirroring
        /// the live result envelope.
        #[serde(skip_serializing_if = "Option::is_none")]
        query: Option<String>,
        /// The complete result set collected for the job.
        results: Vec<SearchResult>,
        /// The job's trace id, echoed verbatim (the client-supplied value
        /// when one was given, never a replacement id).
        #[serde(skip_serializing_if = "Option::is_none")]
        trace_id: Option<String>,
        /// Total number of results collected for the job.
        count: usize,
        /// The engine that served the results, when a single engine answered.
        #[serde(
            skip_serializing_if = "Option::is_none",
            serialize_with = "serialize_engine_opt"
        )]
        engine: Option<SearchEngine>,
        /// Wall-clock duration of the search job in milliseconds.
        duration_ms: u64,
        /// The service-level agreement target for the job in milliseconds.
        sla_ms: u64,
        /// Per-query attribution: every query that contributed to this job.
        queries: Vec<String>,
        /// The raw backend payload for non-search ops (`extract`/`ax`/
        /// `pdf-url`/`pdf-file`); absent for search jobs.
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<serde_json::Value>,
    },
    /// The search failed terminally; final event of a failed stream.
    Error {
        /// Canonical kebab-case error code (see [`ErrorCode`]).
        code: ErrorCode,
        /// Human-readable failure detail.
        message: String,
        /// The engine that failed, when a single engine is implicated.
        #[serde(
            skip_serializing_if = "Option::is_none",
            serialize_with = "serialize_engine_opt"
        )]
        engine: Option<SearchEngine>,
        /// Milliseconds to wait before retrying, when the backend supplied a
        /// `Retry-After` header; `None` when it did not.
        #[serde(skip_serializing_if = "Option::is_none")]
        retry_after_ms: Option<u64>,
    },
}

impl SseEvent {
    /// Whether this event is the terminal `done` event that ends a job's
    /// stream. The worker always emits `done` last, so `error` frames are
    /// non-terminal informational frames that never end a stream on their own.
    #[must_use]
    pub(crate) fn is_terminal(&self) -> bool {
        matches!(self, SseEvent::Done { .. })
    }

    /// The kebab-case SSE event name for a frame's `event:` line — the single
    /// source for the wire event name, matching the serde `event` tag.
    #[must_use]
    pub(crate) fn event_name(&self) -> &'static str {
        match self {
            SseEvent::JobStarted { .. } => "job-started",
            SseEvent::Result(_) => "result",
            SseEvent::EngineEvent { .. } => "engine-event",
            SseEvent::Done { .. } => "done",
            SseEvent::Error { .. } => "error",
        }
    }
}

/// Serialize an engine as its stable lowercase wire id (`SearchEngine::as_str`).
fn serialize_engine<S: Serializer>(
    engine: &SearchEngine,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(engine.as_str())
}

/// Serialize an optional engine as its stable lowercase wire id. Only invoked
/// when the value is `Some` (the field is `skip_serializing_if` `None`).
fn serialize_engine_opt<S: Serializer>(
    engine: &Option<SearchEngine>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match engine {
        Some(engine) => serializer.serialize_str(engine.as_str()),
        None => serializer.serialize_none(),
    }
}

/// Serialize an engine lifecycle kind as its kebab-case wire id.
fn serialize_engine_kind<S: Serializer>(
    kind: &EngineEventKind,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(match kind {
        EngineEventKind::Fallback => "fallback",
        EngineEventKind::RateLimited => "rate-limited",
        EngineEventKind::Captcha => "captcha",
        EngineEventKind::Cooldown => "cooldown",
    })
}

#[cfg(test)]
pub(crate) fn test_result() -> SearchResult {
    SearchResult {
        title: "Example".into(),
        url: "https://example.com".into(),
        snippet: "A snippet".into(),
        position: 3,
        provenance: Provenance::default(),
        domain_authority: 0.8,
        source_type: "web".into(),
        engine: SearchEngine::Brave,
        score: 0.0,
        published_date: None,
        favicon: None,
        mode: EngineMode::Hybrid,
    }
}

#[cfg(test)]
mod tests {
    use super::{SseEvent, test_result};
    use gthings_common::taxonomy::ErrorCode;
    use gthings_search::{EngineEventKind, SearchEngine};
    use serde_json::json;

    #[test]
    fn variants_are_internally_tagged_kebab_case() {
        let cases = [
            (
                SseEvent::JobStarted { trace_id: None },
                json!({"event": "job-started"}),
            ),
            (
                SseEvent::JobStarted {
                    trace_id: Some("t-9".into()),
                },
                json!({"event": "job-started", "trace_id": "t-9"}),
            ),
            (
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
                },
                json!({"event": "done", "results": [], "count": 0, "duration_ms": 0, "sla_ms": 0, "queries": []}),
            ),
            (
                SseEvent::EngineEvent {
                    engine: SearchEngine::Brave,
                    kind: EngineEventKind::RateLimited,
                },
                json!({"event": "engine-event", "engine": "brave", "kind": "rate-limited"}),
            ),
            (
                SseEvent::Error {
                    code: ErrorCode::Captcha,
                    message: "blocked".into(),
                    engine: None,
                    retry_after_ms: None,
                },
                json!({"event": "error", "code": "captcha", "message": "blocked"}),
            ),
            (
                SseEvent::Error {
                    code: ErrorCode::RateLimited,
                    message: "429".into(),
                    engine: Some(SearchEngine::Brave),
                    retry_after_ms: Some(2000),
                },
                json!({"event": "error", "code": "rate-limited", "message": "429", "engine": "brave", "retry_after_ms": 2000}),
            ),
        ];
        for (event, expected) in cases {
            assert_eq!(serde_json::to_value(&event).unwrap(), expected);
        }
    }

    #[test]
    fn result_payload_serializes_search_result_under_event_tag() {
        let value = serde_json::to_value(SseEvent::Result(test_result())).unwrap();
        assert_eq!(value["event"], "result");
        assert_eq!(value["title"], "Example");
        assert_eq!(value["position"], 3);
        assert_eq!(value["engine"], "brave");
    }

    #[test]
    fn done_carries_complete_result_envelope() {
        let event = SseEvent::Done {
            query: Some("rust async".into()),
            results: vec![test_result()],
            trace_id: Some("t-42".into()),
            count: 1,
            engine: Some(SearchEngine::Brave),
            duration_ms: 123,
            sla_ms: 5000,
            queries: vec!["rust async".into()],
            content: None,
        };
        let value = serde_json::to_value(event).unwrap();
        assert_eq!(value["event"], "done");
        assert_eq!(value["query"], "rust async");
        assert_eq!(value["trace_id"], "t-42");
        assert_eq!(value["count"], 1);
        assert_eq!(value["engine"], "brave");
        assert_eq!(value["duration_ms"], 123);
        assert_eq!(value["sla_ms"], 5000);
        assert_eq!(value["queries"][0], "rust async");
        assert_eq!(value["results"][0]["title"], "Example");
        assert_eq!(value["results"][0]["position"], 3);
    }
}
