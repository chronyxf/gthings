//! Integration tests for the `gthings-search` crate.
//!
//! These tests cover type serialization, ordering, and defaults —
//! no browser required.

use gthings_common::provenance::{ExtractionMethod, Provenance};
use gthings_search::{FollowResult, SearchResult};

/// Minimal provenance value used by test helpers.
fn test_provenance() -> Provenance {
    Provenance {
        source_url: "".to_string(),
        method: ExtractionMethod::Search,
        agent: "gthings/test".to_string(),
        accessed_at: chrono::Utc::now(),
        duration_ms: 0,
        derived_from: None,
    }
}

// ---------------------------------------------------------------------------
// SearchResult
// ---------------------------------------------------------------------------

#[test]
fn test_search_result_serde() {
    let result = SearchResult {
        title: "Rust Programming".into(),
        url: "https://www.rust-lang.org".into(),
        snippet: "A language empowering everyone to build reliable software.".into(),
        position: 1,
        domain_authority: 0.0,
        provenance: test_provenance(),
    };

    let json = serde_json::to_string(&result).unwrap();
    assert!(json.contains("\"title\""));
    assert!(json.contains("\"url\""));
    assert!(json.contains("\"position\":1"));

    let parsed: SearchResult = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.title, "Rust Programming");
    assert_eq!(parsed.url, "https://www.rust-lang.org");
    assert_eq!(parsed.position, 1);
}

#[test]
fn test_search_result_ordering() {
    let mut results = [
        SearchResult {
            title: "C".into(),
            url: "https://example.com/c".into(),
            snippet: "".into(),
            position: 3,
            domain_authority: 0.0,
            provenance: test_provenance(),
        },
        SearchResult {
            title: "A".into(),
            url: "https://example.com/a".into(),
            snippet: "".into(),
            position: 1,
            domain_authority: 0.0,
            provenance: test_provenance(),
        },
        SearchResult {
            title: "B".into(),
            url: "https://example.com/b".into(),
            snippet: "".into(),
            position: 2,
            domain_authority: 0.0,
            provenance: test_provenance(),
        },
    ];

    results.sort_by_key(|r| r.position);
    assert_eq!(results[0].position, 1);
    assert_eq!(results[1].position, 2);
    assert_eq!(results[2].position, 3);
    assert_eq!(results[0].title, "A");
}

#[test]
fn test_empty_search_results() {
    let results: Vec<SearchResult> = Vec::new();
    let json = serde_json::to_string(&results).unwrap();
    assert_eq!(json, "[]");

    let parsed: Vec<SearchResult> = serde_json::from_str(&json).unwrap();
    assert!(parsed.is_empty());
}

// ---------------------------------------------------------------------------
// FollowResult
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// SearchResult — malformed / edge-case JSON parsing
// ---------------------------------------------------------------------------

#[test]
fn test_search_result_parse_valid() {
    let json = r#"[
        {"title":"Rust","url":"https://rust-lang.org","snippet":"Safe","position":1},
        {"title":"Cargo","url":"https://crates.io","snippet":"Packages","position":2}
    ]"#;
    let results: Vec<SearchResult> = serde_json::from_str(json).unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].title, "Rust");
    assert_eq!(results[0].position, 1);
    assert_eq!(results[1].title, "Cargo");
    assert_eq!(results[1].position, 2);
}

#[test]
fn test_search_result_parse_empty_array() {
    let results: Vec<SearchResult> = serde_json::from_str("[]").unwrap();
    assert!(results.is_empty());
}

#[test]
fn test_search_result_parse_required_fields() {
    // All required fields provided; matches the shape produced by the JS.
    let json = r#"[
        {"title":"Rust","url":"https://rust-lang.org","snippet":"Safe","position":1}
    ]"#;
    let results: Vec<SearchResult> = serde_json::from_str(json).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].title, "Rust");
    assert_eq!(results[0].url, "https://rust-lang.org");
    assert_eq!(results[0].snippet, "Safe");
    assert_eq!(results[0].position, 1);
}

#[test]
fn test_search_result_parse_malformed() {
    let err = serde_json::from_str::<Vec<SearchResult>>("not json");
    let _ = err.unwrap_err();
}

#[test]
fn test_search_result_parse_partial_object() {
    // Should gracefully handle extra fields and partial data.
    let json = r#"[
        {"title":"A","url":"https://a.co","snippet":"desc","position":1,"extra":"ignored"}
    ]"#;
    let results: Vec<SearchResult> = serde_json::from_str(json).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].title, "A");
    assert_eq!(results[0].position, 1);
    assert!(results[0].domain_authority == 0.0);
}

// ---------------------------------------------------------------------------
// FollowResult
// ---------------------------------------------------------------------------

#[test]
fn test_follow_result_serde() {
    use gthings_common::pagination::Pagination;

    let pagination = Some(Pagination {
        offset: 0,
        returned_len: 49,
        total_len: Some(49),
        truncated: false,
        continuation_token: None,
    });
    let result = FollowResult {
        url: "https://example.com".into(),
        title: "Example Domain".into(),
        content: "This domain is for use in illustrative examples.".into(),
        error: String::new(),
        provenance: test_provenance(),
        pagination: pagination.clone(),
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
