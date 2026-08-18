//! Integration tests for the `gthings-search` crate.
//!
//! These tests cover type serialization, ordering, and defaults —
//! no browser required.

use gthings_common::provenance::{ExtractionMethod, Provenance};
use gthings_search::FollowResult;

/// Minimal provenance value used by test helpers.
fn test_provenance() -> Provenance {
    Provenance {
        source_url: "".to_string(),
        method: ExtractionMethod::Search,
        agent: "gthings/test".to_string(),
        accessed_at: chrono::Utc::now(),
        duration_ms: 0,
    }
}

// ---------------------------------------------------------------------------
// FollowResult
// ---------------------------------------------------------------------------

#[test]
fn test_follow_result_serde() {
    use gthings_common::pagination::Pagination;

    let pagination = Some(Pagination { truncated: false });
    let result = FollowResult {
        url: "https://example.com".into(),
        title: "Example Domain".into(),
        content: "This domain is for use in illustrative examples.".into(),
        error: String::new(),
        provenance: test_provenance(),
        pagination: pagination.clone(),
        quality_flags: Vec::new(),
    };

    let json = serde_json::to_string(&result).unwrap();
    assert!(json.contains("\"url\""));
    assert!(json.contains("\"title\""));
    assert!(json.contains("\"content\""));
    assert!(json.contains("\"pagination\""));

    let parsed: FollowResult = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.url, "https://example.com");
    assert_eq!(parsed.title, "Example Domain");
    assert_eq!(parsed.pagination, pagination);
    assert!(parsed.error.is_empty());
}
