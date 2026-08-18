//! Structured stderr telemetry.
//!
//! The serve daemon writes JSON lines to **stderr** — the only channel the Go
//! side observes. Each event carries an ISO-8601 timestamp, a level, a
//! `trace_id`, and an arbitrary event payload. The `trace_id` is inherited
//! from the W3C `TRACEPARENT` env var (when the caller provides one) or
//! generated as a fresh UUID v4.

use std::io::{self, Write};
use std::sync::OnceLock;

use serde::Serialize;

/// Env var carrying a W3C `traceparent` header (`version-traceid-parentid-flags`).
pub const TRACEPARENT_ENV: &str = "TRACEPARENT";

/// Length (in hex digits) of a W3C trace-id segment.
const TRACE_ID_HEX_LEN: usize = 32;

/// One JSON line written to stderr.
#[derive(Debug, Clone, Serialize)]
pub struct StderrEvent {
    /// ISO-8601 timestamp (UTC), e.g. `2026-08-05T12:00:00.000000Z`.
    pub ts: String,
    /// Event level (`info`, `warn`, `error`, ...).
    pub level: String,
    /// Correlation id for the current job/request.
    pub trace_id: String,
    /// Arbitrary structured event payload.
    pub event: serde_json::Value,
}

impl StderrEvent {
    /// Build a telemetry event, timestamping it now.
    #[must_use]
    pub fn new(level: impl Into<String>, trace_id: String, event: impl Serialize) -> Self {
        let event = match serde_json::to_value(event) {
            Ok(value) => value,
            Err(e) => {
                tracing::debug!("telemetry: failed to serialize event: {e}");
                serde_json::Value::Null
            }
        };
        Self {
            ts: chrono::Utc::now().to_rfc3339(),
            level: level.into(),
            trace_id,
            event,
        }
    }

    /// Serialize and write a single JSON line to stderr.
    ///
    /// # Errors
    ///
    /// Returns [`io::Error`] if stderr cannot be written.
    pub fn emit(&self) -> io::Result<()> {
        let mut line = serde_json::to_string(self)?;
        line.push('\n');
        let mut stderr = io::stderr().lock();
        stderr.write_all(line.as_bytes())?;
        stderr.flush()
    }
}

/// Resolve the current trace id: `TRACEPARENT` trace-id segment if valid,
/// otherwise a freshly generated UUID v4 (cached per process).
#[must_use]
pub fn trace_id() -> String {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| {
        std::env::var(TRACEPARENT_ENV)
            .ok()
            .as_deref()
            .and_then(parse_traceparent)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
    })
    .clone()
}

/// Extract the trace-id segment from a W3C `traceparent` value.
///
/// Format: `version-traceid-parentid-flags` where `traceid` is 32 lowercase
/// hex digits (uppercase tolerated).
fn parse_traceparent(traceparent: &str) -> Option<String> {
    let trace_id = traceparent.split('-').nth(1)?;
    (trace_id.len() == TRACE_ID_HEX_LEN && trace_id.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| trace_id.to_string())
}

#[cfg(test)]
mod tests {
    use super::{StderrEvent, parse_traceparent};

    #[test]
    fn parses_valid_traceparent() {
        let tp = "00-4bf92f3577b34cd9d9b6d1c8b3a8b2b8-00f067aa0ba902b7-01";
        assert_eq!(
            parse_traceparent(tp).as_deref(),
            Some("4bf92f3577b34cd9d9b6d1c8b3a8b2b8")
        );
    }

    #[test]
    fn rejects_malformed_traceparent() {
        assert_eq!(parse_traceparent("00-xyz-00f067aa0ba902b7-01"), None);
        assert_eq!(parse_traceparent("no-hyphens"), None);
        assert_eq!(parse_traceparent(""), None);
        assert_eq!(parse_traceparent("00-short-00f067aa0ba902b7-01"), None);
    }

    #[test]
    fn event_serializes_to_expected_shape() {
        let event = StderrEvent::new(
            "info",
            "trace-1".to_string(),
            serde_json::json!({"op": "job"}),
        );
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(value["level"], "info");
        assert_eq!(value["trace_id"], "trace-1");
        assert_eq!(value["event"]["op"], "job");
        assert!(value["ts"].as_str().is_some_and(|ts| !ts.is_empty()));
    }
}
