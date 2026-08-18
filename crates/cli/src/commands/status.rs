//! `gthings status` — detect browser connection.
//!
//! Readiness is env-only (PROPOSAL §9): the CDP endpoint is resolved through
//! the shared [`crate::util::env_ws_url`] helper — `GTHINGS_CDP_WS_URL`. The
//! `--cdp-url` flag is deliberately not consulted: readiness is driven by
//! environment configuration, while that flag belongs to connect-oriented
//! subcommands.

use gthings_cdp::CdpError;
use gthings_common::taxonomy::ErrorCode;

use crate::util::{UniversalFlags, emit_success, env_ws_url, print_error};

/// Status: resolve the readiness endpoint, no connection needed.
///
/// Honors `GTHINGS_CDP_WS_URL` (env-only) and never opens a connection.
/// Exit-code contract: `0` when a browser is running, `0` with `status:
/// stopped` when none is found, `1` on unexpected errors.
pub(crate) async fn cmd_status(flags: &UniversalFlags) -> i32 {
    // Env-only readiness — same `GTHINGS_CDP_WS_URL` branch the shared
    // resolution chain (`connect::resolve_ws_url`) uses; the `--cdp-url` flag
    // is skipped so status stays independent of CLI connection overrides.
    if let Some(url) = env_ws_url() {
        // Only the env URL is known — do not fabricate browser/version fields.
        let value = serde_json::json!({
            "status": "running",
            "ws_url": url,
            "browser": serde_json::Value::Null,
            "version": serde_json::Value::Null,
        });
        emit_success(flags, value);
        return 0;
    }

    let p = flags.effective_cdp_port();

    match gthings_cdp::detect(p).await {
        Ok(browser) => {
            let mut value = serde_json::json!({});
            value["status"] = serde_json::json!("running");
            value["ws_url"] = serde_json::json!(browser.ws_url);
            value["browser"] = serde_json::json!(browser.browser);
            value["version"] = serde_json::json!(browser.version);
            emit_success(flags, value);
            0
        }
        Err(CdpError::BrowserNotFound { .. }) => {
            emit_success(flags, serde_json::json!({ "status": "stopped" }));
            0
        }
        Err(e) => {
            print_error(
                ErrorCode::BrowserNotFound,
                &e.to_string(),
                "Check browser debugging port",
            );
            1
        }
    }
}
