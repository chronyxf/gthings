// End-to-end tests for gthings agent workflows.

mod common;

use crate::common::*;

// Test 1: Clean stale browser + launch fresh one
#[test]
fn test_cleanup_and_launch() {
    // Kill any leftover from previous runs
    stop_existing_browser(&gthings_bin());

    assert!(wait_for_port(10), "Port 9222 should become free within 10s");

    // Search auto-launches the browser
    let (json, _) = run_gthings(&["--json", "search", "query", "Rust", "--count", "1"]);
    let results = json
        .get("results")
        .or_else(|| json.get("data"))
        .and_then(|r| r.as_array());
    assert!(results.is_some(), "Search should return results: {}", json);

    let (status_json, _) = run_gthings(&["--json", "browser", "status"]);
    assert_eq!(
        status_json["status"], "running",
        "Browser should be running after search, got: {}",
        status_json
    );
    assert!(
        status_json["pid"].as_u64().is_some(),
        "Browser PID should be present"
    );
}

// Test 2: Search returns structured results
#[test]
fn test_search_query_returns_results() {
    let (json, _) = run_gthings(&["--json", "search", "query", "Rust async", "--count", "2"]);
    let results = json
        .get("results")
        .or_else(|| json.get("data"))
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
    let (json, _) = run_gthings(&[
        "--json",
        "follow",
        "url",
        "https://www.rust-lang.org",
        "--max",
        "5000",
    ]);
    let data = json.get("data").unwrap_or(&json);
    let content_length = data
        .get("total_length")
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
    let (json, _) = run_gthings(&[
        "--json",
        "follow",
        "batch",
        "https://www.rust-lang.org",
        "https://github.com/tokio-rs/tokio",
        "--max",
        "3000",
    ]);
    let pages = json
        .as_array()
        .or_else(|| json.get("pages").and_then(|p| p.as_array()))
        .or_else(|| json.get("data").and_then(|d| d.as_array()));
    match pages {
        Some(pages) => {
            assert!(!pages.is_empty(), "Should have at least 1 page");
            for (i, page) in pages.iter().enumerate() {
                println!("Page {}: url={:?}", i + 1, page.get("url"));
            }
        }
        None => panic!("Unexpected JSON for batch: {}", json),
    }
}

// Test 5: Final cleanup
#[test]
fn test_cleanup_browser() {
    let (stop_json, _) = run_gthings(&["--json", "browser", "stop"]);
    assert!(
        stop_json.get("pid").or(stop_json.get("status")).is_some(),
        "Stop should return status: {}",
        stop_json
    );

    std::thread::sleep(std::time::Duration::from_millis(500));

    let (status_json, _) = run_gthings(&["--json", "browser", "status"]);
    assert_eq!(
        status_json["status"], "stopped",
        "Browser should be stopped, got: {}",
        status_json
    );
}

// Test 6: No state file is written (stateless design)
#[test]
fn test_no_state_file_created() {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    let state_path = std::path::Path::new(&home).join(".gthings").join("browser.json");
    assert!(!state_path.exists(), "State file should not exist (stateless design)");
}

// Test 7: dismiss_allow_debugging_dialog does not panic
#[test]
fn test_dismiss_dialog_no_panic() {
    // dismiss_allow_debugging_dialog should not panic even when no dialog is shown
    gthings_cdp::browser::dismiss_allow_debugging_dialog();
}

// Test 8: Browser detection on unused port
#[test]
fn test_browser_detection() {
    // Use a non-standard port so we don't interfere with user's browser
    let port = 29997;

    // Should not find a browser on unused port
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(async {
        gthings_cdp::Browser::find_existing(None, port).await
    });
    assert!(result.is_none(), "Should not find browser on unused port {}", port);
}

// Test 9: Follow extracts content from example.com
#[test]
fn test_follow_extracts_content() {
    let (json, _) = crate::common::run_gthings(&[
        "--json", "follow", "url", "https://example.com",
    ]);
    assert!(json.get("content").is_some(), "Follow should extract content");
    let content = json["content"].as_str().unwrap_or("");
    assert!(!content.is_empty(), "Content should not be empty");
    assert!(
        content.contains("Example Domain") || content.contains("example"),
        "Content should contain expected text"
    );
}
