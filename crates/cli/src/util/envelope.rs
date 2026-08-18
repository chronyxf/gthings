//! Output envelope construction for AI-agent consumption.
//!
//! Every command produces a `{status, data, error}` envelope so that agents
//! (and humans) have a single parse path regardless of success or failure.

use gthings_common::envelope::{Envelope, ErrorBody};
use gthings_common::taxonomy::ErrorCode;
use serde_json::Value;

use crate::util::{OutputFormat, UniversalFlags, format_output};

/// Emit the envelope to stdout, applying the output format and optional query.
/// The optional `query` is applied *after* the envelope is built, allowing
/// callers to extract specific sub-fields (e.g. `--query .data`).
pub(crate) fn emit_output(
    value: Option<Value>,
    error: Option<(ErrorCode, &str, &str)>,
    format: OutputFormat,
    query: Option<&str>,
) {
    let envelope = build_envelope(value.as_ref(), error);
    let formatted = format_output(&envelope, format, query);
    println!("{formatted}");
}

/// Emit a success envelope carrying `value` using the universal flags.
pub(crate) fn emit_success(flags: &UniversalFlags, value: Value) {
    emit_output(
        Some(value),
        None,
        flags.resolved_output(),
        flags.query.as_deref(),
    );
}

/// Emit an error envelope using the universal flags.
pub(crate) fn emit_error(flags: &UniversalFlags, code: ErrorCode, detail: &str, hint: &str) {
    emit_output(
        None,
        Some((code, detail, hint)),
        flags.resolved_output(),
        flags.query.as_deref(),
    );
}

/// Build the standard `{status, data, error}` JSON envelope, with an additive
/// `trace_id` field (inherited from `TRACEPARENT` when the caller provides one).
///
/// The envelope shape comes from [`gthings_common::envelope::Envelope`] so the
/// CLI, serve daemon, and Go integration all agree on the wire format; the
/// `trace_id` is appended for job correlation across the whole pipeline.
pub(crate) fn build_envelope(
    data: Option<&Value>,
    error: Option<(ErrorCode, &str, &str)>,
) -> Value {
    let envelope = match error {
        Some((code, detail, hint)) => {
            let body = ErrorBody {
                code,
                detail: detail.to_string(),
                hint: Some(hint.to_string()),
            };
            Envelope::<Value>::error(body)
        }
        None => Envelope::ok(data.cloned().unwrap_or(Value::Null)),
    };
    let mut value = serde_json::to_value(&envelope).unwrap_or(Value::Null);
    value["trace_id"] = Value::String(gthings_common::telemetry::trace_id());
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_envelope_ok_includes_trace_id() {
        let envelope = build_envelope(Some(&json!({"ok": true})), None);
        assert_eq!(envelope["status"], "ok");
        assert_eq!(envelope["data"], json!({"ok": true}));
        assert_eq!(envelope["error"], Value::Null);
        assert!(
            envelope["trace_id"].as_str().is_some_and(|t| !t.is_empty()),
            "envelope should carry a non-empty trace_id"
        );
    }

    #[test]
    fn test_envelope_error_keeps_canonical_shape() {
        let envelope = build_envelope(
            None,
            Some((ErrorCode::BrowserNotFound, "no browser", "start one")),
        );
        assert_eq!(envelope["status"], "error");
        assert_eq!(envelope["data"], Value::Null);
        assert_eq!(envelope["error"]["code"], "browser-not-found");
        assert_eq!(envelope["error"]["detail"], "no browser");
        assert_eq!(envelope["error"]["hint"], "start one");
        assert!(
            envelope["trace_id"].as_str().is_some_and(|t| !t.is_empty()),
            "error envelopes must also carry trace_id"
        );
    }

    #[test]
    fn test_envelope_ndjson_is_single_compact_line() {
        let envelope = build_envelope(Some(&json!({"ok": true})), None);
        let line = format_output(&envelope, OutputFormat::NdJson, None);
        assert!(line.starts_with('{') && line.ends_with('}'), "got: {line}");
        assert!(
            !line.contains('\n'),
            "nd-json must be a single line, got: {line}"
        );
        let parsed: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(parsed["status"], "ok");
        assert!(
            parsed["trace_id"].as_str().is_some_and(|t| !t.is_empty()),
            "nd-json envelope should include trace_id"
        );
    }
}
