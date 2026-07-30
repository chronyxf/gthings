//! Shared connection helpers for CLI subcommands.

use gthings_cdp::{Session, detect};

use crate::commands::helpers::UniversalFlags;

/// Resolve the CDP WebSocket URL.
///
/// Priority: `--cdp-url` flag → `GTHINGS_CDP_WS_URL` env var → detection via port.
pub(crate) async fn resolve_ws_url(flags: &UniversalFlags) -> Result<String, i32> {
    if let Some(url) = &flags.cdp_url {
        return Ok(url.clone());
    }

    if let Ok(url) = std::env::var("GTHINGS_CDP_WS_URL") {
        if !url.is_empty() {
            return Ok(url);
        }
    }

    let p = flags.cdp_port;
    let browser = detect(p).await.map_err(|e| {
        print_error(
            "BROWSER_NOT_FOUND",
            &format!("No browser found on port {p}: {e}"),
            "Open Dia or Chrome with --remote-debugging-port=9222",
        );
        1
    })?;

    Ok(browser.ws_url)
}

/// Print a machine-readable error JSON to stderr.
pub(crate) fn print_error(code: &str, detail: &str, hint: &str) {
    let err = serde_json::json!({
        "error": code,
        "detail": detail,
        "hint": hint,
    });
    eprintln!("{err}");
}

/// Detect browser → connect → return Session.
pub(crate) async fn connect(flags: &UniversalFlags) -> Result<Session, i32> {
    let ws_url = resolve_ws_url(flags).await?;

    tracing::info!("Connecting to browser at {}", ws_url);

    let timeout = Some(std::time::Duration::from_secs(flags.timeout));
    Session::connect(&ws_url, timeout).await.map_err(|e| {
        print_error(
            "CONNECTION_FAILED",
            &e.to_string(),
            "Verify WebSocket URL is accessible",
        );
        1
    })
}
