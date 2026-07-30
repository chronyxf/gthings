//! End-to-end tests for the gthings CLI binary.
//!
//! Tests marked `#[ignore]` require a running Chrome instance with
//! remote debugging enabled. Run manually with:
//!   cargo test --test e2e -- --ignored
//!
//! Chrome tests use a dedicated port (29992-29994) to avoid interfering
//! with user browsing sessions.

use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Locate the `gthings` binary.
fn gthings_binary() -> PathBuf {
    // CARGO_BIN_EXE_gthings is set when running `cargo test -p gthings`.
    // For workspace-level `cargo test`, fall back to target/debug/gthings.
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_gthings") {
        return PathBuf::from(path);
    }
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("target/debug/gthings");
    if !path.exists() {
        path.set_extension("exe");
    }
    path
}

/// Run `gthings` with the given args, return stdout.
#[allow(dead_code)]
fn run_gthings(args: &[&str]) -> String {
    run_gthings_with_env(args, &[])
}

/// Run `gthings` with custom environment variables, return stdout.
fn run_gthings_with_env(args: &[&str], envs: &[(&str, &str)]) -> String {
    let mut cmd = Command::new(gthings_binary());
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let output = cmd
        .args(args)
        .output()
        .expect("failed to run gthings binary");
    assert!(
        output.status.success(),
        "gthings exited with code {:?}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr),
    );
    String::from_utf8_lossy(&output.stdout).to_string()
}

/// Launch headless Chrome on the given debugging port.
fn launch_chrome(port: u16) -> Child {
    let chrome = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
    Command::new(chrome)
        .args([
            "--headless",
            &format!("--remote-debugging-port={port}"),
            "--no-first-run",
            "--disable-fre",
            "--disable-search-engine-choice-screen",
            "--user-data-dir=/tmp/gthings-e2e-chrome",
        ])
        .spawn()
        .expect("failed to launch Chrome. Install Google Chrome first.")
}

/// Poll until `port` accepts TCP connections (up to `timeout`).
fn wait_for_port(port: u16, timeout: Duration) -> bool {
    let start = Instant::now();
    let addr: String = format!("127.0.0.1:{port}");
    while start.elapsed() < timeout {
        if TcpStream::connect_timeout(&addr.parse().unwrap(), Duration::from_millis(200)).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

/// Gracefully stop a Chrome child process.
fn kill_chrome(mut child: Child) {
    let _ = child.kill();
    let _ = child.wait();
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn test_status_shows_stopped_when_no_browser() {
    let stdout = run_gthings_with_env(&["status", "--json"], &[("GTHINGS_CDP_PORT", "29999")]);
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON from status");
    assert_eq!(
        value["data"]["status"], "stopped",
        "expected stopped status"
    );
}

#[test]
#[ignore]
fn test_search_via_chrome() {
    const PORT: u16 = 29992;
    let chrome = launch_chrome(PORT);
    assert!(
        wait_for_port(PORT, Duration::from_secs(15)),
        "Chrome did not start on port {PORT}"
    );

    let port_str = PORT.to_string();
    let stdout = run_gthings_with_env(
        &["search", "rust programming", "--count", "3", "--json"],
        &[("GTHINGS_CDP_PORT", &port_str)],
    );
    let results: Vec<serde_json::Value> = serde_json::from_str(&stdout).expect("valid JSON array");
    assert_eq!(results.len(), 3, "expected 3 search results");
    for r in &results {
        let title = r["title"].as_str().unwrap_or("");
        let url = r["url"].as_str().unwrap_or("");
        assert!(!title.is_empty(), "result title should be non-empty");
        assert!(!url.is_empty(), "result url should be non-empty");
    }

    kill_chrome(chrome);
}

#[test]
#[ignore]
fn test_follow_extracts_content() {
    const PORT: u16 = 29993;
    let chrome = launch_chrome(PORT);
    assert!(
        wait_for_port(PORT, Duration::from_secs(15)),
        "Chrome did not start on port {PORT}"
    );

    let port_str = PORT.to_string();
    let stdout = run_gthings_with_env(
        &[
            "follow",
            "https://example.com",
            "--max-chars",
            "500",
            "--json",
        ],
        &[("GTHINGS_CDP_PORT", &port_str)],
    );
    let result: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON object");
    let content = result["content"].as_str().unwrap_or("");
    assert!(
        content.contains("Example Domain"),
        "content should contain 'Example Domain', got: {content}"
    );

    kill_chrome(chrome);
}

#[test]
#[ignore]
fn test_batch_search() {
    const PORT: u16 = 29994;
    let chrome = launch_chrome(PORT);
    assert!(
        wait_for_port(PORT, Duration::from_secs(15)),
        "Chrome did not start on port {PORT}"
    );

    let port_str = PORT.to_string();
    let stdout = run_gthings_with_env(
        &["batch", "rust", "python", "go", "--count", "2", "--json"],
        &[("GTHINGS_CDP_PORT", &port_str)],
    );
    let results: Vec<Vec<serde_json::Value>> =
        serde_json::from_str(&stdout).expect("valid JSON array of arrays");
    assert_eq!(results.len(), 3, "expected 3 query result sets");
    for (i, batch) in results.iter().enumerate() {
        assert_eq!(
            batch.len(),
            2,
            "query {i}: expected 2 results, got {}",
            batch.len()
        );
    }

    kill_chrome(chrome);
}
