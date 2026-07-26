//! Shared connection helpers for CLI subcommands.

use gthings_cdp::{CdpError, Session, detect};

/// Port from `GTHINGS_CDP_PORT` env var (default 9222).
pub(crate) fn port() -> u16 {
    std::env::var("GTHINGS_CDP_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(9222)
}

/// Print a machine-readable error JSON to stderr.
pub(crate) fn print_error(code: &str, detail: &str, hint: &str) {
    let err = serde_json::json!({
        "error": code,
        "detail": detail,
        "hint": hint,
    });
    eprintln!("{}", err);
}

/// Detect browser → connect → return Session.
pub(crate) async fn connect() -> Result<Session, i32> {
    let p = port();

    // `detect` internally checks GTHINGS_CDP_WS_URL first (fast path),
    // then probes the CDP port via HTTP /json/version, /json, /json/list,
    // and finally DevToolsActivePort file scan.
    let browser = detect(p).await.map_err(|_| {
        print_error(
            "BROWSER_NOT_FOUND",
            &format!("No browser found on port {p}"),
            "Open Dia or Chrome with --remote-debugging-port=9222",
        );
        1
    })?;

    tracing::info!("Connecting to browser at {}", browser.ws_url);

    Session::connect(&browser.ws_url).await.map_err(|e| {
        print_error(
            "CONNECTION_FAILED",
            &e.to_string(),
            "Verify WebSocket URL is accessible",
        );
        1
    })
}

/// Map common CDP errors to machine-readable error JSON.
pub(crate) fn on_cdp_error(e: &CdpError) {
    match e {
        CdpError::NavigationTimeout { .. } => {
            print_error(
                "NAVIGATION_TIMEOUT",
                &e.to_string(),
                "Check network connectivity or URL",
            );
        }
        _ => {
            print_error(
                "SEARCH_FAILED",
                &e.to_string(),
                "Retry with different arguments",
            );
        }
    }
}
