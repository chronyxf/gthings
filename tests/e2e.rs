// End-to-end tests for gthings agent workflows.
// Tests use the real CLI binary and a persistent Dia/Chrome browser.
// Browser is shared across tests 1-4, cleaned up in test 5.
//
// Run: cargo test --test e2e -- --test-threads=1

mod common;

use crate::common::*;

// Test 1: Clean stale browser + launch fresh one
#[test]
fn test_cleanup_and_launch() {
    // Kill any leftover from previous runs
    stop_existing_browser(&gthings_bin());
    
    // Wait for port to be free
    assert!(wait_for_port(10), "Port 9222 should become free within 10s");
    
    // Run a search — this auto-launches the browser
    let (json, _) = run_gthings(&["--json", "search", "query", "Rust", "--count", "1"]);
    let results = json.get("results").or_else(|| json.get("data"))
        .and_then(|r| r.as_array());
    assert!(results.is_some(), "Search should return results: {}", json);
    
    // Browser should now be running
    let (status_json, _) = run_gthings(&["--json", "browser", "status"]);
    assert_eq!(status_json["status"], "running",
        "Browser should be running after search, got: {}", status_json);
    assert!(status_json["pid"].as_u64().is_some(),
        "Browser PID should be present");
}

// Test 2: Search returns structured results
#[test]
fn test_search_query_returns_results() {
    let (json, _) = run_gthings(&["--json", "search", "query", "Rust async", "--count", "2"]);
    let results = json.get("results").or_else(|| json.get("data"))
        .and_then(|r| r.as_array());
    match results {
        Some(results) => {
            assert!(!results.is_empty(), "Should have at least 1 result");
            if let Some(first) = results.first() {
                assert!(first.get("title").is_some(), "Result should have title");
                assert!(first.get("url").is_some(), "Result should have url");
            }
        }
        None => panic!("Unexpected JSON: {}", json),
    }
}

// Test 3: Follow URL extracts content
#[test]
fn test_follow_url_extracts_content() {
    let (json, _) = run_gthings(&["--json", "follow", "url", 
        "https://www.rust-lang.org", "--max", "5000"]);
    let data = json.get("data").unwrap_or(&json);
    let content_length = data.get("total_length")
        .or_else(|| data.get("returned_length"))
        .and_then(|v| v.as_u64());
    assert!(content_length.is_some(), "Should report content length");
    if let Some(len) = content_length {
        assert!(len > 100, "Content should be >100 chars, got {}", len);
    }
}

// Test 4: Batch follow multiple URLs
#[test]
fn test_follow_batch_multiple_urls() {
    let (json, _) = run_gthings(&["--json", "follow", "batch",
        "https://www.rust-lang.org",
        "https://github.com/tokio-rs/tokio",
        "--max", "3000"]);
    let pages = json.as_array()
        .or_else(|| json.get("pages").and_then(|p| p.as_array()))
        .or_else(|| json.get("data").and_then(|d| d.as_array()));
    match pages {
        Some(pages) => {
            assert!(!pages.is_empty(), "Should have at least 1 page");
            for (i, page) in pages.iter().enumerate() {
                println!("Page {}: url={:?}", i+1, page.get("url"));
            }
        }
        None => panic!("Unexpected JSON for batch: {}", json),
    }
}

// Test 5: Final cleanup
#[test]
fn test_cleanup_browser() {
    let (stop_json, _) = run_gthings(&["--json", "browser", "stop"]);
    assert!(stop_json.get("pid").or(stop_json.get("status")).is_some(),
        "Stop should return status: {}", stop_json);
    
    std::thread::sleep(std::time::Duration::from_millis(500));
    
    let (status_json, _) = run_gthings(&["--json", "browser", "status"]);
    assert_eq!(status_json["status"], "stopped",
        "Browser should be stopped, got: {}", status_json);
}
