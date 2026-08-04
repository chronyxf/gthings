//! Shared CLI helpers: universal flags, output formatting, and query filtering.
//!
//! Output pipeline: command → [`emit_output`] → [`format_output`] → stdout.
//! Every command returns a `{status, data, error}` envelope so agents have one
//! parse path regardless of success or failure.

use serde_json::Value;
use std::borrow::Cow;
use std::sync::Arc;
use std::sync::OnceLock;

/// Output format for command results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum OutputFormat {
    /// Human-readable text output (default).
    Text,
    /// Pretty-printed JSON.
    Json,
    /// Compact JSON lines (one JSON value per line).
    NdJson,
}

/// Universal flags shared across all gthings subcommands.
#[derive(Debug, clap::Args)]
pub(crate) struct UniversalFlags {
    /// Output format. Overridden by --json for backward compatibility.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub output: OutputFormat,

    /// JMESPath-like query filter (dot notation, e.g. '.title' or '.[].url'). Applied after the output envelope is built.
    #[arg(long, value_name = "QUERY")]
    pub query: Option<String>,

    /// Override CDP port (default: 9222, or GTHINGS_CDP_PORT env var).
    #[arg(
        long,
        value_name = "PORT",
        default_value_t = 9222,
        env = "GTHINGS_CDP_PORT"
    )]
    pub cdp_port: u16,

    /// Override CDP WebSocket URL (takes priority over port detection).
    #[arg(long, value_name = "URL")]
    pub cdp_url: Option<String>,

    /// Timeout in seconds for CDP calls and extraction (default: 30). Connection setup may take longer.
    #[arg(long, value_name = "SECS", default_value_t = 30)]
    pub timeout: u64,

    /// Increase verbosity (can be repeated: -v -v for debug, -v -v -v for trace).
    #[arg(short = 'v', long, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Suppress non-error output.
    #[arg(short = 'q', long)]
    pub quiet: bool,

    /// Backward-compatible alias for --output json.
    #[arg(long)]
    pub json: bool,
}

impl UniversalFlags {
    /// Resolve the effective output format, honoring the --json backward-compat alias.
    pub(crate) fn resolved_output(&self) -> OutputFormat {
        if self.json {
            OutputFormat::Json
        } else {
            self.output
        }
    }

    /// Merge non-default values from `other` into self.
    /// Used to propagate top-level flags into subcommand-level flags.
    pub(crate) fn merge_from(&mut self, other: &UniversalFlags) {
        if other.cdp_port != 9222 {
            self.cdp_port = other.cdp_port;
        }
        if other.cdp_url.is_some() {
            self.cdp_url.clone_from(&other.cdp_url);
        }
        if other.timeout != 30 {
            self.timeout = other.timeout;
        }
        if other.verbose > 0 {
            self.verbose = other.verbose;
        }
        if other.quiet {
            self.quiet = other.quiet;
        }
        if other.json {
            self.json = other.json;
        }
        if other.output != OutputFormat::Text {
            self.output = other.output;
        }
        if other.query.is_some() {
            self.query.clone_from(&other.query);
        }
    }

    /// Determine the tracing log level based on verbosity, quiet flag, and output format.
    ///
    /// - `--quiet` or JSON output → only ERROR
    /// - NdJson output → WARN (reduce noise for streaming)
    /// - default (no flags) → INFO
    /// - `-v` → DEBUG
    /// - `-vv` (or more) → TRACE
    pub(crate) fn tracing_level(&self) -> &str {
        if self.quiet || self.resolved_output() == OutputFormat::Json {
            "error"
        } else if self.resolved_output() == OutputFormat::NdJson {
            match self.verbose {
                0 => "warn",
                1 => "debug",
                _ => "trace",
            }
        } else {
            match self.verbose {
                0 => "info",
                1 => "debug",
                _ => "trace",
            }
        }
    }
}

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

/// Apply a simple dot-notation JMESPath-like query to a JSON value.
///
/// Supported syntax:
/// - `.field` — access object key
/// - `.[].field` — iterate array, access field on each element
/// - `.field[].subfield` — access object key (array), iterate, access subfield
/// - `[].field` — same as `.[].field`
fn apply_query(value: &Value, query: &str) -> Value {
    let segments = parse_query_segments(query);
    let results = apply_segments(value, &segments);
    if results.len() == 1 {
        results.into_iter().next().unwrap_or(Value::Null)
    } else {
        Value::Array(results)
    }
}

#[derive(Debug, Clone)]
enum QuerySegment {
    /// Access a named field on an object.
    Field(String),
    /// Iterate over an array and continue with remaining segments on each element.
    Iterate,
    /// Access a field then iterate over the resulting array.
    FieldThenIterate(String),
}

fn parse_query_segments(query: &str) -> Vec<QuerySegment> {
    let mut segments = Vec::new();
    // Strip leading dot if present
    let query = query.strip_prefix('.').unwrap_or(query);
    if query.is_empty() {
        return segments;
    }
    for part in query.split('.') {
        if part.is_empty() {
            continue;
        }
        if part == "[]" {
            segments.push(QuerySegment::Iterate);
        } else if let Some(stripped) = part.strip_suffix("[]") {
            if stripped.is_empty() {
                segments.push(QuerySegment::Iterate);
            } else {
                segments.push(QuerySegment::FieldThenIterate(stripped.to_string()));
            }
        } else {
            segments.push(QuerySegment::Field(part.to_string()));
        }
    }
    segments
}

fn apply_segments(value: &Value, segments: &[QuerySegment]) -> Vec<Value> {
    if segments.is_empty() {
        return vec![value.clone()];
    }

    let (first, rest) = (&segments[0], &segments[1..]);

    match first {
        QuerySegment::Field(name) => {
            if let Some(next) = value.get(name) {
                apply_segments(next, rest)
            } else {
                vec![]
            }
        }
        QuerySegment::Iterate => {
            if let Some(arr) = value.as_array() {
                arr.iter()
                    .flat_map(|item| apply_segments(item, rest))
                    .collect()
            } else {
                vec![]
            }
        }
        QuerySegment::FieldThenIterate(name) => {
            if let Some(next) = value.get(name) {
                if let Some(arr) = next.as_array() {
                    arr.iter()
                        .flat_map(|item| apply_segments(item, rest))
                        .collect()
                } else {
                    apply_segments(next, rest)
                }
            } else {
                vec![]
            }
        }
    }
}

/// Unified output for AI-agent consumption.
///
/// Every command produces a `{status, data, error}` envelope so that agents
/// (and humans) have a single parse path regardless of success or failure.
/// The optional `query` is applied *after* the envelope is built, allowing
/// callers to extract specific sub-fields (e.g. `--query .data`).
pub(crate) fn emit_output(
    value: Option<Value>,
    error: Option<(&str, &str, &str)>,
    format: OutputFormat,
    query: Option<&str>,
) {
    let envelope = build_envelope(value, error);
    let formatted = format_output(&envelope, format, query);
    println!("{formatted}");
}

/// Build the standard `{status, data, error}` JSON envelope.
#[allow(clippy::needless_pass_by_value)]
fn build_envelope(data: Option<Value>, error: Option<(&str, &str, &str)>) -> Value {
    let error_obj = error.map(
        |(code, detail, hint)| serde_json::json!({"code": code, "detail": detail, "hint": hint}),
    );
    serde_json::json!({
        "status": if error_obj.is_some() { "error" } else { "ok" },
        "data": data,
        "error": error_obj,
    })
}
/// Shared HTTP client (lazily initialized, connection-pooled).
pub(crate) fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (compatible; gthings/0.5)")
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_else(|e| {
                tracing::error!(error = %e, "Failed to build HTTP client");
                std::process::exit(1);
            })
    })
}

/// Produce a concise human-readable summary of a JSON value.
///
/// Long strings (>200 chars) are truncated with an ellipsis.
/// Short strings are returned as a borrowed slice to avoid allocation.
#[allow(clippy::incompatible_msrv)]
fn text_summary(value: &Value) -> Cow<'_, str> {
    match value {
        Value::String(s) => {
            if s.len() > 200 {
                // Truncate and allocate a new shortened string.
                format!("{}...", &s[..s.floor_char_boundary(200)]).into()
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

/// Attempt a clean disconnect when the Arc has unique ownership.
/// If other references still exist, the disconnect is silently skipped.
pub(crate) async fn disconnect_session(session: Arc<gthings_cdp::Session>) {
    if let Ok(s) = Arc::try_unwrap(session) {
        if let Err(e) = s.disconnect().await {
            tracing::warn!("disconnect failed: {e}");
        }
    }
}

/// Connect, wrap in `Arc<Session>`, run the async function, then disconnect.
/// Returns the exit code from `f`, or the connection error code.
pub(crate) async fn with_session<F, Fut>(flags: &UniversalFlags, f: F) -> i32
where
    F: FnOnce(Arc<gthings_cdp::Session>) -> Fut,
    Fut: std::future::Future<Output = i32>,
{
    let session = match crate::commands::connect(flags).await {
        Ok(s) => s,
        Err(c) => return c,
    };
    let arc_session = Arc::new(session);
    let code = f(Arc::clone(&arc_session)).await;
    disconnect_session(arc_session).await;
    code
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

    #[test]
    fn test_resolved_output_json_flag() {
        let flags = UniversalFlags {
            output: OutputFormat::Text,
            query: None,
            cdp_port: 9222,
            cdp_url: None,
            timeout: 30,
            verbose: 0,
            quiet: false,
            json: true,
        };
        assert_eq!(flags.resolved_output(), OutputFormat::Json);
    }

    #[test]
    fn test_resolved_output_explicit() {
        let flags = UniversalFlags {
            output: OutputFormat::NdJson,
            query: None,
            cdp_port: 9222,
            cdp_url: None,
            timeout: 30,
            verbose: 0,
            quiet: false,
            json: false,
        };
        assert_eq!(flags.resolved_output(), OutputFormat::NdJson);
    }
}
