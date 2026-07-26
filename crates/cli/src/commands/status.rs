//! `gthings status` — detect browser connection.

use gthings_cdp::{CdpError, detect};

use crate::commands::{port, print_error};

/// Status: detect only, no connection needed.
pub(crate) async fn cmd_status(json: bool) -> i32 {
    match detect(port()).await {
        Ok(browser) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "status": "running",
                        "ws_url": browser.ws_url,
                        "browser": browser.browser,
                        "version": browser.version,
                    })
                );
            } else {
                println!("Browser: {} {}", browser.browser, browser.version);
                println!("WebSocket URL: {}", browser.ws_url);
            }
            0
        }
        Err(CdpError::BrowserNotFound { .. }) => {
            if json {
                let output = serde_json::json!({
                    "status": "stopped"
                });
                println!("{output}");
            } else {
                println!("Browser is NOT running");
            }
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
