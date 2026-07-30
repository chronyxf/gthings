use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::error::GthingsError;

/// Internal JSON structure for continuation token serialization.
#[derive(Serialize, Deserialize)]
struct EncodedToken {
    url: String,
    offset: usize,
    max_chars: usize,
}

/// Parameters controlling extraction offset and maximum length.
///
/// Passed to every `Extractor::extract()` call. Default values
/// (`offset = 0`, `max_chars = usize::MAX`) mean "extract everything".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractParams {
    pub offset: usize,
    pub max_chars: usize,
}

impl Default for ExtractParams {
    fn default() -> Self {
        Self {
            offset: 0,
            max_chars: usize::MAX,
        }
    }
}

/// Pagination state returned inside every `Article`.
///
/// Tells the consumer whether the content was truncated, what range
/// was returned, and (optionally) how to fetch the next chunk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pagination {
    pub offset: usize,
    pub returned_len: usize,
    pub total_len: Option<usize>,
    pub truncated: bool,
    pub continuation_token: Option<String>,
}

/// Encode a continuation token that captures URL, next offset, and `max_chars`.
///
/// The token is a base64-encoded JSON object: `{"url":..., "offset":..., "max_chars":...}`.
///
/// # Errors
///
/// Returns a [`GthingsError::Parse`] if JSON serialization fails (should never
/// happen for this plain data).
pub fn encode_continuation_token(
    url: &str,
    next_offset: usize,
    max_chars: usize,
) -> Result<String, GthingsError> {
    let token = EncodedToken {
        url: url.to_string(),
        offset: next_offset,
        max_chars,
    };
    let json = serde_json::to_string(&token)
        .map_err(|e| GthingsError::Parse(format!("continuation token serialization: {e}")))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(json.as_bytes()))
}

/// Decode a continuation token back into `(url, offset, max_chars)`.
///
/// # Errors
///
/// Returns [`GthingsError::Parse`] if the token is not valid base64,
/// not valid UTF-8, or not valid JSON with the expected structure.
pub fn decode_continuation_token(token: &str) -> Result<(String, usize, usize), GthingsError> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(token)
        .map_err(|e| GthingsError::Parse(format!("invalid base64 continuation token: {e}")))?;
    let json = String::from_utf8(bytes)
        .map_err(|e| GthingsError::Parse(format!("invalid UTF-8 in continuation token: {e}")))?;
    let token: EncodedToken = serde_json::from_str(&json)
        .map_err(|e| GthingsError::Parse(format!("invalid JSON in continuation token: {e}")))?;
    Ok((token.url, token.offset, token.max_chars))
}

/// Build a [`Pagination`] from extraction parameters and content lengths.
///
/// Computes `truncated`, sets `returned_len`, and creates a continuation token
/// when the content was truncated.
///
/// # Errors
///
/// Returns [`GthingsError::Parse`] if the continuation token cannot be
/// serialized (should never happen for this plain data).
pub fn build_pagination(
    params: &ExtractParams,
    url: &str,
    total_len: usize,
    returned_len: usize,
) -> Result<Pagination, GthingsError> {
    let truncated =
        params.offset.saturating_add(params.max_chars) < total_len && params.max_chars > 0;
    let continuation_token = if truncated {
        let next_offset = params.offset.saturating_add(params.max_chars);
        Some(encode_continuation_token(
            url,
            next_offset,
            params.max_chars,
        )?)
    } else {
        None
    };
    Ok(Pagination {
        offset: params.offset,
        returned_len,
        total_len: Some(total_len),
        truncated,
        continuation_token,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_decode_roundtrip() {
        let token = encode_continuation_token("https://example.com/article", 5000, 2000).unwrap();
        let (url, offset, max_chars) = decode_continuation_token(&token).unwrap();
        assert_eq!(url, "https://example.com/article");
        assert_eq!(offset, 5000);
        assert_eq!(max_chars, 2000);
    }

    #[test]
    fn test_decode_invalid_base64() {
        let err = decode_continuation_token("!!!not-base64!!!").unwrap_err();
        assert!(matches!(err, GthingsError::Parse(_)));
        assert!(
            err.to_string().contains("base64"),
            "expected error message to mention 'base64', got: {}",
            err
        );
    }

    #[test]
    fn test_decode_invalid_json() {
        // base64 of "not-json"
        let token = base64::engine::general_purpose::STANDARD.encode(b"not-json");
        let err = decode_continuation_token(&token).unwrap_err();
        assert!(matches!(err, GthingsError::Parse(_)));
        assert!(
            err.to_string().contains("JSON"),
            "expected error message to mention 'JSON', got: {}",
            err
        );
    }

    // -- build_pagination tests -------------------------------------------

    #[test]
    fn test_build_pagination_truncated() {
        // offset=0, max_chars=100, total_len=1000 → truncated=true, continuation_token present
        let params = ExtractParams {
            offset: 0,
            max_chars: 100,
        };
        let result = build_pagination(&params, "https://example.com", 1000, 100).unwrap();
        assert!(result.truncated);
        assert!(result.continuation_token.is_some());
        assert_eq!(result.offset, 0);
        assert_eq!(result.returned_len, 100);
        assert_eq!(result.total_len, Some(1000));
    }

    #[test]
    fn test_build_pagination_not_truncated() {
        // offset=0, max_chars=1000, total_len=100 (content shorter than max) → truncated=false
        let params = ExtractParams {
            offset: 0,
            max_chars: 1000,
        };
        let result = build_pagination(&params, "https://example.com", 100, 100).unwrap();
        assert!(!result.truncated);
        assert!(result.continuation_token.is_none());
    }

    #[test]
    fn test_build_pagination_token_roundtrip() {
        // Encode a token, decode it, verify fields match
        let params = ExtractParams {
            offset: 0,
            max_chars: 500,
        };
        let result = build_pagination(&params, "https://example.com/page", 2000, 500).unwrap();
        let token = result.continuation_token.unwrap();
        let (url, next_offset, max_chars) = decode_continuation_token(&token).unwrap();
        assert_eq!(url, "https://example.com/page");
        assert_eq!(next_offset, 500); // offset + max_chars
        assert_eq!(max_chars, 500);
    }

    #[test]
    fn test_build_pagination_offset_boundary() {
        // offset + max_chars saturating (not overflowing) — test with usize::MAX
        let params = ExtractParams {
            offset: usize::MAX - 10,
            max_chars: 20,
        };
        let result = build_pagination(&params, "https://example.com", usize::MAX, 20).unwrap();
        // Should not panic; truncated should be false since we got everything
        assert!(!result.truncated);
    }
}
