use serde_json::Value;
use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;

/// Trace recorder for AI agent step-level debugging.
/// Writes JSONL records to a file, one event per line.
#[allow(dead_code)]
pub struct TraceWriter {
    path: PathBuf,
    file: Option<std::fs::File>,
    start: Instant,
}

/// Structured trace event for a single step
#[derive(Debug, Clone, serde::Serialize)]
pub struct TraceEvent {
    pub ts: u64,                    // Unix timestamp (ms)
    pub session: String,            // Session identifier (per CLI invocation)
    pub step: u32,                  // Step number within the session
    pub tool: String,               // Tool/command name (e.g. "search", "follow")
    pub action: String,             // Specific action (e.g. "browser_launch", "tab_create", "navigate", "extract")
    pub url: Option<String>,        // URL being operated on
    pub duration_ms: u64,           // Duration of this step
    pub input: Option<Value>,       // Input parameters (truncated to 200 chars)
    pub output: Option<Value>,      // Output summary (truncated)
    pub error: Option<String>,      // Error message if failed
    pub result_count: Option<u32>,  // Number of results found
    pub content_length: Option<u64>,// Content length extracted
    pub quality_ok: Option<bool>,   // Quality gate result
}

impl TraceWriter {
    /// Create a new trace writer. Opens the file immediately.
    pub fn new(path: &str) -> std::io::Result<Self> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        Ok(TraceWriter {
            path: PathBuf::from(path),
            file: Some(file),
            start: Instant::now(),
        })
    }

    /// Record a single trace event as a JSONL line
    pub fn record(&mut self, event: &TraceEvent) {
        if let Some(ref mut file) = self.file {
            let json = serde_json::to_string(event).unwrap_or_default();
            let _ = writeln!(file, "{}", json);
            let _ = file.flush();
        }
    }

    /// Record a simple step event (convenience method)
    pub fn step(
        &mut self,
        session: &str,
        step_num: u32,
        tool: &str,
        action: &str,
        url: Option<&str>,
        duration_ms: u64,
        input: Option<Value>,
        output: Option<Value>,
        error: Option<&str>,
    ) {
        let result_count = output.as_ref()
            .and_then(|o| o.get("result_count").or(o.get("count")).and_then(|v| v.as_u64()).map(|v| v as u32));
        let content_length = output.as_ref()
            .and_then(|o| o.get("content_length").or(o.get("total_length")).and_then(|v| v.as_u64()));
        let quality_ok = output.as_ref()
            .and_then(|o| o.pointer("/quality/is_ok").or(o.pointer("/data/quality/is_ok")).and_then(|v| v.as_bool()));

        let event = TraceEvent {
            ts: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            session: session.to_string(),
            step: step_num,
            tool: tool.to_string(),
            action: action.to_string(),
            url: url.map(|s| s.to_string()),
            duration_ms,
            input: input.map(|v| truncate_value(v, 200)),
            output: output.map(|v| truncate_value(v, 500)),
            error: error.map(|s| s.to_string()),
            result_count,
            content_length,
            quality_ok,
        };
        self.record(&event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn test_trace_writer_creates_file() {
        let path = "/tmp/test-trace.jsonl";
        let _ = std::fs::remove_file(path);

        let mut writer = TraceWriter::new(path).unwrap();
        writer.step("session-1", 1, "search", "navigate",
            Some("https://example.com"), 100, None, None, None);

        let mut contents = String::new();
        std::fs::File::open(path).unwrap().read_to_string(&mut contents).unwrap();
        assert!(contents.contains("search"));
        assert!(contents.contains("navigate"));
        assert!(contents.contains("session-1"));
        assert!(contents.contains("100"));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_truncate_value_string() {
        let v = Value::String("hello world very long string".into());
        let truncated = truncate_value(v, 10);
        assert!(truncated.as_str().unwrap().contains("truncated"));
    }

    #[test]
    fn test_truncate_value_short_string() {
        let v = Value::String("hello".into());
        let truncated = truncate_value(v, 100);
        assert_eq!(truncated.as_str().unwrap(), "hello");
    }
}

/// Truncate a JSON value to a maximum string length
fn truncate_value(v: Value, max_len: usize) -> Value {
    match v {
        Value::String(s) => {
            if s.len() > max_len {
                Value::String(format!("{}... (truncated, {} total)", &s[..max_len], s.len()))
            } else {
                Value::String(s)
            }
        }
        Value::Object(map) => {
            let mut new_map = serde_json::Map::new();
            for (k, v) in map.into_iter().take(10) {
                new_map.insert(k, truncate_value(v, max_len));
            }
            Value::Object(new_map)
        }
        Value::Array(arr) => {
            let truncated: Vec<Value> = arr.into_iter().take(5).map(|v| truncate_value(v, max_len)).collect();
            Value::Array(truncated)
        }
        other => other,
    }
}
