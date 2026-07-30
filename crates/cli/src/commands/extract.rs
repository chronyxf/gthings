//! `gthings extract` — content extraction from URLs.
//!
//! Uses HTTP-based extraction (reqwest) rather than CDP browser navigation,
//! so each call is inherently isolated — no shared tab session.

use crate::commands::{UniversalFlags, emit_output};
use gthings_common::pagination::ExtractParams;
use gthings_extraction::dispatch::AutoExtractor;

/// Extract content from any URL using auto-detection.
pub(crate) async fn cmd_extract(
    flags: &UniversalFlags,
    url: &str,
    max_chars: usize,
    offset: usize,
) -> i32 {
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (compatible; gthings/0.5)")
        .timeout(std::time::Duration::from_secs(flags.timeout))
        .build()
        .expect("reqwest Client::builder() with default config should never fail");

    let extractor = AutoExtractor::new(client);
    let params = ExtractParams { offset, max_chars };
    match extractor.extract(url, params).await {
        Ok(article) => {
            let mut value = serde_json::json!(article);
            if let Some(obj) = value.as_object_mut() {
                obj.insert("original_url".to_string(), serde_json::json!(url));
                obj.insert("body_status".to_string(), serde_json::json!("ok"));
            }
            emit_output(
                Some(value),
                None,
                flags.resolved_output(),
                flags.query.as_deref(),
            );
            0
        }
        Err(e) => {
            emit_output(
                None,
                Some((
                    "EXTRACT_FAILED",
                    &e.to_string(),
                    "Check URL and connectivity",
                )),
                flags.resolved_output(),
                flags.query.as_deref(),
            );
            1
        }
    }
}
