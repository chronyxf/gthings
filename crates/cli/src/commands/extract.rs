//! `gthings extract` — content extraction from URLs.
//!
//! Uses HTTP-based extraction (reqwest) rather than CDP browser navigation,
//! so each call is inherently isolated — no shared tab session.

use crate::util::{UniversalFlags, emit_error, emit_success};
use gthings_common::pagination::ExtractParams;
use gthings_common::taxonomy::ErrorCode;
use gthings_extraction::Extractor;
use gthings_extraction::dispatch::AutoExtractor;

/// Extract content from any URL using auto-detection.
pub(crate) async fn cmd_extract(
    flags: &UniversalFlags,
    url: &str,
    max_chars: usize,
    offset: usize,
) -> i32 {
    let extractor = AutoExtractor::new(crate::util::http_client());
    let params = ExtractParams { offset, max_chars };
    match extractor.extract(url.to_string(), params).await {
        Ok(article) => {
            let mut value = serde_json::json!(article);
            if let Some(obj) = value.as_object_mut() {
                obj.insert("original_url".to_string(), serde_json::json!(url));
                obj.insert("body_status".to_string(), serde_json::json!("ok"));
            }
            emit_success(flags, value);
            0
        }
        Err(e) => {
            emit_error(
                flags,
                ErrorCode::ExtractFailed,
                &e.to_string(),
                "Check URL and connectivity",
            );
            1
        }
    }
}
