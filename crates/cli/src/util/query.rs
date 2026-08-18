//! JMESPath-like query filtering for output values.
//!
//! Supported syntax:
//! - `.field` — access object key
//! - `.[].field` — iterate array, access field on each element
//! - `.field[].subfield` — access object key (array), iterate, access subfield
//! - `[].field` — same as `.[].field`

use serde_json::Value;

/// Apply a simple dot-notation JMESPath-like query to a JSON value.
///
/// Supported syntax:
/// - `.field` — access object key
/// - `.[].field` — iterate array, access field on each element
/// - `.field[].subfield` — access object key (array), iterate, access subfield
/// - `[].field` — same as `.[].field`
pub(crate) fn apply_query(value: &Value, query: &str) -> Value {
    let segments = parse_query_segments(query);
    let results = apply_segments(value, &segments);
    if results.len() == 1 {
        results.into_iter().next().unwrap()
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
        if let Some(stripped) = part.strip_suffix("[]") {
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
