//! Integration tests for the `gthings-search` crate.
//!
//! These tests cover type serialization, ordering, and defaults —
//! no browser required.

use gthings_search::{FollowResult, SearchResult};

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
    let mut results = vec![
        SearchResult {
            title: "C".into(),
            url: "https://example.com/c".into(),
            snippet: "".into(),
            position: 3,
        },
        SearchResult {
            title: "A".into(),
            url: "https://example.com/a".into(),
            snippet: "".into(),
            position: 1,
        },
        SearchResult {
            title: "B".into(),
            url: "https://example.com/b".into(),
            snippet: "".into(),
            position: 2,
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

#[test]
fn test_follow_result_serde() {
    let result = FollowResult {
        url: "https://example.com".into(),
        title: "Example Domain".into(),
        content: "This domain is for use in illustrative examples.".into(),
        truncated: false,
        error: String::new(),
    };

    let json = serde_json::to_string(&result).unwrap();
    assert!(json.contains("\"url\""));
    assert!(json.contains("\"title\""));
    assert!(json.contains("\"content\""));
    assert!(json.contains("\"truncated\":false"));

    let parsed: FollowResult = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.url, "https://example.com");
    assert_eq!(parsed.title, "Example Domain");
    assert!(!parsed.truncated);
    assert!(parsed.error.is_empty());
}


