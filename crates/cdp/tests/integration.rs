//! Integration tests for `gthings-cdp` — browser detection and error display.
//!
//! These tests verify browser detection and error display
//! without requiring a running browser.

use gthings_cdp::CdpError;

// ---------------------------------------------------------------------------
// Error display
// ---------------------------------------------------------------------------

#[test]
fn test_error_display_variants() {
    let err = CdpError::BrowserNotFound { port: 9222 };
    assert_eq!(format!("{err}"), "Browser not found on port 9222");

    let err = CdpError::ConnectionFailed {
        detail: "connection refused".into(),
    };
    assert_eq!(format!("{err}"), "Connection failed: connection refused");

    let err = CdpError::CdpCallFailed {
        method: "Page.navigate".into(),
        detail: "timeout".into(),
    };
    assert_eq!(
        format!("{err}"),
        "CDP call Page.navigate failed: timeout"
    );

    let err = CdpError::NavigationTimeout {
        url: "https://example.com".into(),
        timeout: 30,
    };
    assert_eq!(
        format!("{err}"),
        "Navigation timeout: https://example.com did not load within 30s"
    );

    let err = CdpError::Json(serde_json::from_str::<()>("invalid").unwrap_err());
    assert!(format!("{err}").contains("expected"));

    let err = CdpError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "file missing"));
    assert!(format!("{err}").contains("file missing"));
}
