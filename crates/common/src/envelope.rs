//! Shared `{status, data, error}` response envelope.
//!
//! This is the single canonical envelope for the CLI, the serve daemon, and
//! the Go integration, so every producer (and every consumer) agrees on shape.

use serde::{Deserialize, Serialize};

use crate::taxonomy::ErrorCode;

/// The structured `error` slot of an [`Envelope`].
///
/// Mirrors the shape produced by the CLI today: `{"code", "detail", "hint"}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    /// Canonical wire code (see [`ErrorCode`], kebab-case).
    pub code: ErrorCode,
    /// Human-readable explanation.
    pub detail: String,
    /// Optional remediation guidance. Omitted (not `null`) when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl ErrorBody {
    /// Build a body from a canonical code and a detail message.
    #[must_use]
    pub fn new(code: ErrorCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
            hint: None,
        }
    }
}

/// The standard `{status, data, error}` envelope.
///
/// `status` is `"ok"` when `error` is `None` and `"error"` otherwise.
/// Generic over the data payload; defaults to [`serde_json::Value`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope<T = serde_json::Value> {
    /// `"ok"` or `"error"`.
    pub status: String,
    /// Payload on success; `None` on error.
    pub data: Option<T>,
    /// Structured error on failure; `None` on success.
    pub error: Option<ErrorBody>,
}

impl<T> Envelope<T> {
    /// Build a success envelope carrying `data`.
    #[must_use]
    pub fn ok(data: T) -> Self {
        Self {
            status: "ok".to_string(),
            data: Some(data),
            error: None,
        }
    }

    /// Build an error envelope with no data payload.
    #[must_use]
    pub fn error(body: ErrorBody) -> Self {
        Self {
            status: "error".to_string(),
            data: None,
            error: Some(body),
        }
    }

    /// `true` if the envelope reports success.
    #[must_use]
    pub const fn is_ok(&self) -> bool {
        self.error.is_none()
    }

    /// `true` if the envelope reports failure.
    #[must_use]
    pub const fn is_err(&self) -> bool {
        self.error.is_some()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{Envelope, ErrorBody};
    use crate::taxonomy::ErrorCode;

    #[test]
    fn ok_envelope_serializes_to_canonical_shape() {
        let envelope = Envelope::ok(json!({"results": [1, 2, 3]}));
        let value = serde_json::to_value(&envelope).unwrap();
        assert_eq!(value["status"], "ok");
        assert_eq!(value["data"], json!({"results": [1, 2, 3]}));
        assert_eq!(value["error"], serde_json::Value::Null);
        assert!(envelope.is_ok());
        assert!(!envelope.is_err());
    }

    #[test]
    fn error_envelope_serializes_with_hint() {
        let body = ErrorBody {
            code: ErrorCode::RateLimited,
            detail: "slow down".to_string(),
            hint: Some("retry after backoff".to_string()),
        };
        let envelope = Envelope::<serde_json::Value>::error(body);
        let value = serde_json::to_value(&envelope).unwrap();
        assert_eq!(value["status"], "error");
        assert_eq!(value["data"], serde_json::Value::Null);
        assert_eq!(value["error"]["code"], "rate-limited");
        assert_eq!(value["error"]["detail"], "slow down");
        assert_eq!(value["error"]["hint"], "retry after backoff");
        assert!(envelope.is_err());
    }

    #[test]
    fn error_envelope_omits_hint_when_absent() {
        let body = ErrorBody {
            code: ErrorCode::Timeout,
            detail: "too slow".to_string(),
            hint: None,
        };
        let envelope = Envelope::<serde_json::Value>::error(body);
        let value = serde_json::to_value(&envelope).unwrap();
        assert_eq!(value["error"]["code"], "timeout");
        assert_eq!(value["error"]["detail"], "too slow");
        assert!(value["error"].get("hint").is_none());
    }

    #[test]
    fn generic_payload_type_round_trips() {
        let envelope = Envelope::ok(vec![1_u64, 2, 3]);
        let json = serde_json::to_string(&envelope).unwrap();
        let back: Envelope<Vec<u64>> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, envelope);
        assert_eq!(back.data, Some(vec![1, 2, 3]));
    }
}
