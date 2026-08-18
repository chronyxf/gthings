//! Rate-limit (HTTP 429) detection and `Retry-After` propagation.

use reqwest::Response;

use crate::article::ExtractionError;

/// Decide whether an HTTP status indicates rate-limiting (429) and, if so,
/// build a `RateLimited` error carrying the parsed `Retry-After` delay.
///
/// Kept pure (status + optional header) so the `Retry-After` propagation can
/// be unit-tested without a live [`Response`].
pub(crate) fn rate_limit_status(
    status: u16,
    retry_after: Option<&reqwest::header::HeaderValue>,
    detail: String,
) -> Result<(), ExtractionError> {
    if status != 429 {
        return Ok(());
    }
    let retry_after =
        match retry_after {
            Some(v) => {
                let s = v
                    .to_str()
                    .map_err(|_| ExtractionError::Parse("non-UTF8 Retry-After header".into()))?;
                Some(s.trim().parse::<u64>().map_err(|e| {
                    ExtractionError::Parse(format!("invalid Retry-After value: {e}"))
                })?)
            }
            None => None,
        };
    Err(ExtractionError::RateLimited {
        detail,
        retry_after,
    })
}

/// Check if the HTTP response indicates rate-limiting (429) and return
/// a `RateLimited` error with the parsed `Retry-After` header if so.
pub(crate) fn check_rate_limit(resp: &Response, detail: String) -> Result<(), ExtractionError> {
    rate_limit_status(
        resp.status().as_u16(),
        resp.headers().get("retry-after"),
        detail,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rate_limit_status_429_carries_retry_after() {
        use reqwest::header::HeaderValue;

        // A 429 must surface as `RateLimited` with the `Retry-After` value
        // intact so callers (e.g. a serve job executor) can honor it.
        let err = rate_limit_status(
            429,
            Some(&HeaderValue::from_static("42")),
            "boom".to_string(),
        )
        .unwrap_err();
        match err {
            ExtractionError::RateLimited {
                detail,
                retry_after,
            } => {
                assert_eq!(detail, "boom");
                assert_eq!(retry_after, Some(42));
            }
            other => panic!("expected RateLimited, got {other:?}"),
        }
    }

    #[test]
    fn test_rate_limit_status_429_without_header() {
        let err = rate_limit_status(429, None, "boom".to_string()).unwrap_err();
        match err {
            ExtractionError::RateLimited { retry_after, .. } => {
                assert_eq!(retry_after, None);
            }
            other => panic!("expected RateLimited, got {other:?}"),
        }
    }

    #[test]
    fn test_rate_limit_status_non_429_is_ok() {
        use reqwest::header::HeaderValue;

        rate_limit_status(200, None, "fine".to_string()).unwrap();
        // A Retry-After header on a non-429 response is irrelevant.
        rate_limit_status(
            503,
            Some(&HeaderValue::from_static("60")),
            "fine".to_string(),
        )
        .unwrap();
    }

    #[test]
    fn test_rate_limit_status_invalid_retry_after_is_parse_error() {
        use reqwest::header::HeaderValue;

        let err = rate_limit_status(
            429,
            Some(&HeaderValue::from_static("soon")),
            "boom".to_string(),
        )
        .unwrap_err();
        assert!(matches!(err, ExtractionError::Parse(_)), "got {err:?}");
    }
}
