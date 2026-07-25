//! Integration tests for gthings-cdp browser detection and lifecycle.
//!
//! These tests verify stateless browser detection, DevToolsActivePort parsing,
//! and the dismiss dialog helper — without requiring a running browser.

use gthings_cdp::Browser;

const UNUSED_PORT: u16 = 29999;

#[test]
fn test_discover_ws_url_no_browser() {
    // Should return None when no browser is running
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(async {
        Browser::discover_ws_url(None, UNUSED_PORT).await
    });
    assert!(result.is_none(), "Should not find WS URL on unused port");
}

#[test]
fn test_find_existing_no_browser() {
    // Should return None when no browser is running
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(async {
        Browser::find_existing(None, UNUSED_PORT).await
    });
    assert!(result.is_none(), "Should not find browser on unused port");
}

#[test]
fn test_probe_port_unused() {
    assert!(!Browser::probe_port(UNUSED_PORT), "Unused port should not respond");
}

#[test]
fn test_probe_port_invalid() {
    assert!(!Browser::probe_port(0), "Port 0 should not respond");
}

#[test]
fn test_discover_ws_devtools_port_format() {
    // Verify that discover_ws_url handles malformed DevToolsActivePort correctly
    let tmp = std::env::temp_dir().join("gthings-test-dtap");
    let _ = std::fs::create_dir_all(&tmp);

    // Write invalid content (single line) with unused port
    std::fs::write(tmp.join("DevToolsActivePort"), &format!("{UNUSED_PORT}\n")).unwrap();

    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(async {
        Browser::discover_ws_url(Some(&tmp), UNUSED_PORT).await
    });
    assert!(result.is_none(), "Single-line DevToolsActivePort should return None");

    // Cleanup
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn test_discover_ws_devtools_port_valid() {
    let tmp = std::env::temp_dir().join("gthings-test-dtap-valid");
    let _ = std::fs::create_dir_all(&tmp);

    // Write valid DevToolsActivePort with unused port
    std::fs::write(
        tmp.join("DevToolsActivePort"),
        &format!("{UNUSED_PORT}\n/devtools/browser/test-uuid\n"),
    )
    .unwrap();

    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(async {
        Browser::discover_ws_url(Some(&tmp), UNUSED_PORT).await
    });
    assert_eq!(
        result,
        Some(format!("ws://127.0.0.1:{UNUSED_PORT}/devtools/browser/test-uuid")),
        "Valid DevToolsActivePort should return correct WS URL"
    );

    // Cleanup
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn test_discover_ws_port_mismatch() {
    let tmp = std::env::temp_dir().join("gthings-test-dtap-mismatch");
    let _ = std::fs::create_dir_all(&tmp);

    // Port in file differs from expected port
    std::fs::write(
        tmp.join("DevToolsActivePort"),
        &format!("{}\n/devtools/browser/test-uuid\n", UNUSED_PORT + 1),
    )
    .unwrap();

    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(async {
        Browser::discover_ws_url(Some(&tmp), UNUSED_PORT).await
    });
    assert!(result.is_none(), "Port mismatch should return None");

    let _ = std::fs::remove_dir_all(&tmp);
}
