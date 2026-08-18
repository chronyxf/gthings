//! Output formatting for command results.
//!
//! Renders a JSON value in one of three styles (`Text`, `Json`, `NdJson`),
//! optionally applying a query filter first.

use std::borrow::Cow;

use serde_json::Value;

use crate::util::OutputFormat;
use crate::util::apply_query;

/// Maximum length of a string before `text_summary` truncates it with an ellipsis.
const TEXT_SUMMARY_MAX: usize = 200;

/// Format a JSON value according to the output format and optional query filter.
///
/// When no query is given, the original value is formatted directly (no clone).
/// When a query is provided, the filtered result is formatted instead.
pub(crate) fn format_output(value: &Value, format: OutputFormat, query: Option<&str>) -> String {
    match query {
        // Short-circuit: no query → format the original value (zero-copy).
        None => format_value(value, format),
        // Apply the query filter, then format the result.
        Some(q) => format_value(&apply_query(value, q), format),
    }
}

/// Core formatting: convert a JSON value to its string representation
/// in one of the three output styles.
fn format_value(value: &Value, format: OutputFormat) -> String {
    match format {
        OutputFormat::Text => format_text(value),
        OutputFormat::Json => serde_json::to_string_pretty(value).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "JSON serialization failed");
            String::new()
        }),
        OutputFormat::NdJson => format_ndjson(value),
    }
}

/// Human-readable text representation.
fn format_text(value: &Value) -> String {
    if let Some(s) = value.as_str() {
        s.to_string()
    } else if let Some(n) = value.as_i64() {
        n.to_string()
    } else if let Some(n) = value.as_f64() {
        n.to_string()
    } else if let Some(b) = value.as_bool() {
        b.to_string()
    } else if let Some(arr) = value.as_array() {
        // Array items: one indexed line per element.
        arr.iter()
            .enumerate()
            .map(|(i, v)| format!("[{}] {}", i + 1, text_summary(v)))
            .collect::<Vec<_>>()
            .join("\n")
    } else if let Some(obj) = value.as_object() {
        // Object entries: one "key: value" line per field.
        obj.iter()
            .map(|(k, v)| format!("{}: {}", k, text_summary(v)))
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        serde_json::to_string(value).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "JSON serialization failed");
            String::new()
        })
    }
}

/// Compact JSON-lines format: one JSON value per line.
/// Arrays expand into multiple lines; scalars/objects render as single lines.
fn format_ndjson(value: &Value) -> String {
    if let Some(arr) = value.as_array() {
        arr.iter()
            .filter_map(|v| {
                serde_json::to_string(v)
                    .map_err(|e| tracing::warn!(error = %e, "JSON serialization failed in filter"))
                    .ok()
            })
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        serde_json::to_string(value).unwrap_or_default()
    }
}

/// Produce a concise human-readable summary of a JSON value.
///
/// Long strings (>200 chars) are truncated with an ellipsis.
/// Short strings are returned as a borrowed slice to avoid allocation.
fn text_summary(value: &Value) -> Cow<'_, str> {
    match value {
        Value::String(s) => {
            if s.len() > TEXT_SUMMARY_MAX {
                // Truncate at a char boundary and allocate a new shortened
                // string (is_char_boundary walk-back keeps this MSRV-safe).
                let mut end = TEXT_SUMMARY_MAX;
                while end > 0 && !s.is_char_boundary(end) {
                    end -= 1;
                }
                format!("{}...", &s[..end]).into()
            } else {
                // Borrow the original string — no allocation needed.
                Cow::Borrowed(s.as_str())
            }
        }
        Value::Number(n) => n.to_string().into(),
        Value::Bool(b) => b.to_string().into(),
        Value::Null => Cow::Borrowed("null"),
        Value::Array(a) => format!("[{} items]", a.len()).into(),
        Value::Object(o) => {
            let keys: Vec<&str> = o.keys().map(|k| k.as_str()).collect();
            format!("{{{}}}", keys.join(", ")).into()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_format_output_text_string() {
        let v = json!("hello world");
        assert_eq!(format_output(&v, OutputFormat::Text, None), "hello world");
    }

    #[test]
    fn test_format_output_text_number() {
        let v = json!(42);
        assert_eq!(format_output(&v, OutputFormat::Text, None), "42");
    }

    #[test]
    fn test_format_output_json_pretty() {
        let v = json!({"title": "test", "url": "http://example.com"});
        let result = format_output(&v, OutputFormat::Json, None);
        assert!(result.contains("\"title\""));
        assert!(result.contains("test"));
    }

    #[test]
    fn test_format_output_ndjson_compact() {
        let v = json!({"title": "test"});
        let result = format_output(&v, OutputFormat::NdJson, None);
        assert_eq!(result, r#"{"title":"test"}"#);
    }

    #[test]
    fn test_format_output_ndjson_array() {
        let v = json!([{"url": "a"}, {"url": "b"}]);
        let result = format_output(&v, OutputFormat::NdJson, None);
        assert_eq!(
            result,
            format!("{}\n{}", r#"{"url":"a"}"#, r#"{"url":"b"}"#)
        );
    }

    #[test]
    fn test_query_field_access() {
        let v = json!({"title": "hello", "url": "http://example.com"});
        let result = format_output(&v, OutputFormat::Text, Some(".title"));
        assert_eq!(result, "hello");
    }

    #[test]
    fn test_query_array_iterate_field() {
        let v = json!([{"url": "http://a.com"}, {"url": "http://b.com"}]);
        let result = format_output(&v, OutputFormat::NdJson, Some(".[].url"));
        assert_eq!(
            result,
            format!("{}\n{}", "\"http://a.com\"", "\"http://b.com\"")
        );
    }

    #[test]
    fn test_query_nested_field_iterate() {
        let v = json!({"results": [{"snippet": "one"}, {"snippet": "two"}]});
        let result = format_output(&v, OutputFormat::NdJson, Some(".results[].snippet"));
        assert_eq!(result, format!("{}\n{}", "\"one\"", "\"two\""));
    }

    #[test]
    fn test_query_invalid_path_returns_null() {
        let v = json!({"title": "hello"});
        let result = format_output(&v, OutputFormat::Json, Some(".nonexistent"));
        assert_eq!(result, "[]");
    }

    #[test]
    fn test_query_no_dot_prefix() {
        let v = json!({"title": "hello"});
        let result = format_output(&v, OutputFormat::Text, Some("title"));
        assert_eq!(result, "hello");
    }
}
