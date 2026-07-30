//! `gthings status` — detect browser connection.

use gthings_cdp::{CdpError, detect};

use crate::commands::{UniversalFlags, emit_output, print_error};

/// Status: detect only, no connection needed.
pub(crate) async fn cmd_status(flags: &UniversalFlags) -> i32 {
    let p = crate::commands::port(flags);

    match detect(p).await {
        Ok(browser) => {
            let value = serde_json::json!({
                "status": "running",
                "ws_url": browser.ws_url,
                "browser": browser.browser,
                "version": browser.version,
            });
            emit_output(
                Some(value),
                None,
                flags.resolved_output(),
                flags.query.as_deref(),
            );
            0
        }
        Err(CdpError::BrowserNotFound { .. }) => {
            let value = serde_json::json!({
                "status": "stopped"
            });
            emit_output(
                Some(value),
                None,
                flags.resolved_output(),
                flags.query.as_deref(),
            );
            0
        }
        Err(e) => {
            print_error(
                "DETECT_FAILED",
                &e.to_string(),
                "Check browser debugging port",
            );
            1
        }
    }
}
