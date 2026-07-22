use std::process::Command;

fn gthings_bin() -> std::path::PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_gthings") {
        return std::path::PathBuf::from(path);
    }
    let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for name in &["target/debug/gthings", "target/release/gthings"] {
        let p = base.join(name);
        if p.exists() {
            return p;
        }
    }
    panic!("gthings binary not found. Build first: cargo build -p gthings");
}

fn gthings() -> Command {
    Command::new(gthings_bin())
}

fn daemon_available() -> bool {
    std::env::var("GTHINGS_TEST_DAEMON").is_ok()
}

fn require_daemon() {
    if !daemon_available() {
        eprintln!("Skipping: set GTHINGS_TEST_DAEMON=1 to run e2e tests");
        std::process::exit(0);
    }
}

fn assert_ok(output: &std::process::Output) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "Command failed (exit={})\nSTDERR: {}",
        output.status.code().unwrap_or(-1),
        stderr
    );
}

fn assert_json(output: &std::process::Output) -> serde_json::Value {
    assert_ok(output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).expect("Output should be valid JSON")
}

// ════════════════════════════════════════════════════════════════════════════════
// E2E: DAEMON LIFECYCLE
// ════════════════════════════════════════════════════════════════════════════════

#[test]
#[ignore = "requires GTHINGS_TEST_DAEMON=1"]
fn test_daemon_status() {
    require_daemon();
    let json = assert_json(
        &gthings()
            .args(["--json", "browser", "status"])
            .output()
            .unwrap(),
    );
    assert!(
        json.get("ok").is_some() || json.get("connected").is_some() || json.get("pid").is_some(),
        "Status should report daemon state: {}",
        json
    );
}

#[test]
#[ignore = "requires GTHINGS_TEST_DAEMON=1"]
fn test_browser_eval_1_plus_1() {
    require_daemon();
    let json = assert_json(
        &gthings()
            .args(["--json", "browser", "eval", "1+1"])
            .output()
            .unwrap(),
    );
    let stdout = json.to_string();
    assert!(
        stdout.contains("2"),
        "Eval 1+1 should return 2. Got: {}",
        stdout
    );
}

#[test]
#[ignore = "requires GTHINGS_TEST_DAEMON=1"]
fn test_browser_call_get_version() {
    require_daemon();
    let json = assert_json(
        &gthings()
            .args(["--json", "browser", "call", "Browser.getVersion", "{}"])
            .output()
            .unwrap(),
    );
    let stdout = json.to_string();
    assert!(
        stdout.contains("product") || stdout.contains("version"),
        "getVersion should return product info: {}",
        stdout
    );
}

// ════════════════════════════════════════════════════════════════════════════════
// E2E: SEARCH + FOLLOW — FULL AGENT WORKFLOW
// ════════════════════════════════════════════════════════════════════════════════

#[test]
#[ignore = "requires GTHINGS_TEST_DAEMON=1"]
fn test_search_query_returns_json_with_meta_and_results() {
    require_daemon();
    let json = assert_json(
        &gthings()
            .args(["--json", "search", "query", "test", "--count", "3"])
            .output()
            .unwrap(),
    );
    assert!(
        json.get("meta").is_some(),
        "Search should have 'meta': {}",
        json
    );
    assert!(
        json.get("results").is_some(),
        "Search should have 'results': {}",
        json
    );
    assert!(
        json["meta"].get("total").is_some(),
        "Meta should have 'total'"
    );
    assert!(
        json["meta"].get("duration_ms").is_some(),
        "Meta should have 'duration_ms'"
    );
}

#[test]
#[ignore = "requires GTHINGS_TEST_DAEMON=1"]
fn test_search_harvest_full_structure() {
    require_daemon();
    let json = assert_json(
        &gthings()
            .args([
                "--json",
                "search",
                "harvest",
                "test topic",
                "--count",
                "2",
                "--max",
                "1",
            ])
            .output()
            .unwrap(),
    );

    // Phase 1: search_results
    assert!(
        json.get("search_results").is_some(),
        "Harvest must include search_results"
    );
    let results = json["search_results"].as_array().unwrap();
    if !results.is_empty() {
        assert!(
            results[0].get("title").is_some(),
            "Search result should have title"
        );
        assert!(
            results[0].get("url").is_some(),
            "Search result should have url"
        );
    }

    // Phase 2: read_pages
    assert!(
        json.get("read_pages").is_some(),
        "Harvest must include read_pages"
    );
    let pages = json["read_pages"].as_array().unwrap();
    for page in pages {
        assert!(page.get("success").is_some(), "Page should have success");
        assert!(page.get("url").is_some(), "Page should have url");
        assert!(page.get("quality").is_some(), "Page should have quality");
        assert!(page.get("sections").is_some(), "Page should have sections");
    }

    // Meta
    let meta = json.get("meta").unwrap();
    assert!(meta.get("queries").is_some(), "Meta should have queries");
    assert!(
        meta.get("total_search_results").is_some(),
        "Meta should have total_search_results"
    );
    assert!(
        meta.get("unique_urls").is_some(),
        "Meta should have unique_urls: {}",
        json
    );
    assert!(
        meta.get("pages_followed").is_some(),
        "Meta should have pages_followed"
    );
    assert!(
        meta.get("pages_skipped").is_some(),
        "Meta should have pages_skipped: {}",
        json
    );
    assert!(
        meta.get("duration_ms").is_some(),
        "Meta should have duration_ms"
    );
}

/// Full agent scenario: 3 finance topics via search harvest
#[test]
#[ignore = "requires GTHINGS_TEST_DAEMON=1"]
fn test_multi_topic_harvest_three_topics() {
    require_daemon();

    // This simulates the 3-agent research scenario with a single harvest call
    let json = assert_json(
        &gthings()
            .args([
                "--json",
                "search",
                "harvest",
                "Federal Reserve interest rate 2026",
                "quantum computing finance 2026",
                "ESG investing sustainable finance 2026",
                "--count",
                "3",
                "--max",
                "2",
            ])
            .output()
            .unwrap(),
    );

    // Verify all three topics were searched
    let meta = json.get("meta").unwrap();
    let queries = meta["queries"].as_array().unwrap();
    assert!(
        queries.len() >= 3,
        "Should have 3+ queries. Got: {}",
        queries.len()
    );

    // Verify results exist
    let results = json["search_results"].as_array().unwrap();
    assert!(!results.is_empty(), "Should have search results");

    // Verify pages were followed
    let pages = json["read_pages"].as_array().unwrap();
    if !pages.is_empty() {
        let page = &pages[0];
        // Check critical agent-facing fields
        assert!(page.get("content").is_some(), "Follow should have content");
        assert!(page.get("quality").is_some(), "Follow should have quality");
        assert!(
            page.get("sections").is_some(),
            "Follow should have sections (not empty array!)"
        );

        // Quality gate — check it works
        let quality = page["quality"].as_object().unwrap();
        assert!(quality.contains_key("is_ok"), "Quality should have is_ok");
        assert!(quality.contains_key("score"), "Quality should have score");
        assert!(
            quality.contains_key("reasons"),
            "Quality should have reasons"
        );
        assert!(quality.contains_key("length"), "Quality should have length");

        // Sections — verify structure
        let sections = page["sections"].as_array().unwrap();
        if !sections.is_empty() {
            let section = &sections[0];
            assert!(
                section.get("heading").is_some(),
                "Section should have heading"
            );
            assert!(
                section.get("content").is_some(),
                "Section should have content"
            );
        }
    }
}

// ════════════════════════════════════════════════════════════════════════════════
// E2E: FOLLOW URL — SECTIONS AND QUALITY
// ════════════════════════════════════════════════════════════════════════════════

#[test]
#[ignore = "requires GTHINGS_TEST_DAEMON=1"]
fn test_follow_url_includes_sections_and_quality() {
    require_daemon();
    let json = assert_json(
        &gthings()
            .args([
                "--json",
                "follow",
                "url",
                "https://example.com",
                "--max",
                "500",
            ])
            .output()
            .unwrap(),
    );

    assert!(
        json.get("sections").is_some(),
        "Must include sections field: {}",
        json
    );
    assert!(
        json.get("quality").is_some(),
        "Must include quality field: {}",
        json
    );
    assert!(json.get("content").is_some(), "Must include content field");
    assert!(json.get("success").is_some(), "Must include success field");
    assert!(json.get("url").is_some(), "Must include url field");
    assert!(
        json.get("total_length").is_some(),
        "Must include total_length"
    );
    assert!(json.get("truncated").is_some(), "Must include truncated");
}

#[test]
#[ignore = "requires GTHINGS_TEST_DAEMON=1"]
fn test_follow_url_truncation_flag() {
    require_daemon();
    let json = assert_json(
        &gthings()
            .args([
                "--json",
                "follow",
                "url",
                "https://example.com",
                "--max",
                "10",
            ])
            .output()
            .unwrap(),
    );
    assert!(
        json.get("truncated").is_some(),
        "Should report truncated state: {}",
        json
    );
    assert!(
        json.get("total_length").is_some(),
        "Should report total_length"
    );
}

// ════════════════════════════════════════════════════════════════════════════════
// E2E: SCRAPE — CSS SELECTOR EXTRACTION
// ════════════════════════════════════════════════════════════════════════════════

#[test]
#[ignore = "requires GTHINGS_TEST_DAEMON=1"]
fn test_scrape_h1_selector() {
    require_daemon();
    let json = assert_json(
        &gthings()
            .args([
                "--json",
                "scrape",
                "https://example.com",
                "--selector",
                "h1",
            ])
            .output()
            .unwrap(),
    );
    let stdout = json.to_string();
    assert!(
        stdout.contains("Example"),
        "Scrape h1 should find 'Example'. Got: {}",
        stdout
    );
}

// ════════════════════════════════════════════════════════════════════════════════
// E2E: SCREENSHOT — FILE + JSON OUTPUT
// ════════════════════════════════════════════════════════════════════════════════

#[test]
#[ignore = "requires GTHINGS_TEST_DAEMON=1"]
fn test_screenshot_png_file() {
    require_daemon();
    let tmpdir = std::env::temp_dir().join("gthings-e2e");
    std::fs::create_dir_all(&tmpdir).unwrap();
    let path = tmpdir.join("e2e_screenshot.png");
    let _ = std::fs::remove_file(&path);

    let output = gthings()
        .args([
            "screenshot",
            "https://example.com",
            "--output",
            &path.to_string_lossy(),
        ])
        .output()
        .unwrap();
    assert_ok(&output);
    assert!(path.exists(), "Screenshot file should exist");
    let meta = std::fs::metadata(&path).unwrap();
    assert!(
        meta.len() > 1000,
        "Screenshot should be > 1KB, got {} bytes",
        meta.len()
    );
}

#[test]
#[ignore = "requires GTHINGS_TEST_DAEMON=1"]
fn test_screenshot_json_output() {
    require_daemon();
    let json = assert_json(
        &gthings()
            .args(["--json", "screenshot", "https://example.com", "--json"])
            .output()
            .unwrap(),
    );
    assert!(
        json.get("data").is_some(),
        "JSON screenshot must have 'data': {}",
        json
    );
    assert!(
        json.get("format").is_some(),
        "JSON screenshot must have 'format'"
    );
    assert!(
        json.get("size").is_some(),
        "JSON screenshot must have 'size'"
    );
}

// ════════════════════════════════════════════════════════════════════════════════
// E2E: TRACE CAPTURES ALL COMMANDS
// ════════════════════════════════════════════════════════════════════════════════

#[test]
#[ignore = "requires GTHINGS_TEST_DAEMON=1"]
fn test_trace_records_multi_topic_harvest() {
    require_daemon();
    let tmpdir = std::env::temp_dir().join("gthings-e2e");
    std::fs::create_dir_all(&tmpdir).unwrap();
    let trace = tmpdir.join("e2e_trace.jsonl");
    let _ = std::fs::remove_file(&trace);

    // Run the multi-topic harvest WITH trace
    let output = gthings()
        .args([
            "--trace",
            &trace.to_string_lossy(),
            "--json",
            "search",
            "harvest",
            "topic 1",
            "topic 2",
            "topic 3",
            "--count",
            "2",
            "--max",
            "1",
        ])
        .output()
        .unwrap();
    assert_ok(&output);

    // Verify trace was written
    let content = std::fs::read_to_string(&trace).unwrap();
    assert!(!content.is_empty(), "Trace file must not be empty");

    // Verify trace structure
    let line = content.lines().next().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(line).unwrap();
    assert!(parsed.get("ts").is_some(), "Trace should have ts");
    assert!(parsed.get("session").is_some(), "Trace should have session");
    assert!(parsed.get("tool").is_some(), "Trace should have tool");
    assert!(
        parsed.get("duration_ms").is_some(),
        "Trace should have duration_ms"
    );
    assert!(parsed.get("exit").is_some(), "Trace should have exit");
    assert!(parsed.get("args").is_some(), "Trace should have args");
}
