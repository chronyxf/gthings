//! Shared connection helpers for CLI subcommands.

use gthings_cdp::{DetectedBrowser, Session, detect};
use gthings_common::taxonomy::ErrorCode;

use crate::util::emit_success;
use crate::util::flags::UniversalFlags;

/// Read the `GTHINGS_CDP_WS_URL` env var, treating empty values as unset.
///
/// Shared by [`resolve_ws_url`] and the readiness-only `status` command so the
/// env branch is resolved identically everywhere.
pub(crate) fn env_ws_url() -> Option<String> {
    match std::env::var("GTHINGS_CDP_WS_URL") {
        Ok(url) if !url.is_empty() => Some(url),
        _ => None,
    }
}

/// Emit a success envelope describing a detected browser (`{browser, version,
/// ws_url}`), shared by `status` and `health`.
pub(crate) fn emit_browser_envelope(flags: &UniversalFlags, browser: &DetectedBrowser) {
    let value = serde_json::json!({
        "browser": browser.browser,
        "version": browser.version,
        "ws_url": browser.ws_url,
    });
    emit_success(flags, value);
}

/// Resolve the CDP WebSocket URL.
///
/// Priority: `--cdp-url` flag → `GTHINGS_CDP_WS_URL` env var → detection via
/// port. The detection fallback probes `http://{GTHINGS_CDP_HOST}:{port}` (host
/// resolved from the shared `gthings_common::config` surface, default
/// `127.0.0.1`), so a remote debugging target is reachable without code changes.
pub(crate) async fn resolve_ws_url(flags: &UniversalFlags) -> Result<String, i32> {
    if let Some(url) = &flags.cdp_url {
        return Ok(url.clone());
    }

    if let Some(url) = env_ws_url() {
        return Ok(url);
    }

    let cfg = gthings_common::config::Config::load();
    let p = flags.effective_cdp_port();
    tracing::debug!(host = %cfg.cdp_host, port = p, "detecting CDP endpoint");
    let browser = detect(p).await.map_err(|e| {
        print_error(
            ErrorCode::BrowserNotFound,
            &format!("No browser found on {host}:{p}: {e}", host = cfg.cdp_host),
            "Open Dia or Chrome with --remote-debugging-port=9222",
        );
        1
    })?;

    Ok(browser.ws_url)
}

/// Print a machine-readable error JSON to stderr.
pub(crate) fn print_error(code: ErrorCode, detail: &str, hint: &str) {
    let err = serde_json::json!({
        "error": code.as_str(),
        "detail": detail,
        "hint": hint,
    });
    eprintln!("{err}");
}

/// Detect browser → connect → return Session.
pub(crate) async fn connect(flags: &UniversalFlags) -> Result<Session, i32> {
    let ws_url = resolve_ws_url(flags).await?;

    tracing::info!("Connecting to browser at {}", ws_url);

    let timeout = Some(std::time::Duration::from_secs(flags.effective_timeout()));
    Session::connect(&ws_url, timeout).await.map_err(|e| {
        print_error(
            ErrorCode::ConnectionFailed,
            &e.to_string(),
            "Verify WebSocket URL is accessible",
        );
        1
    })
}
