//! `gthings health` — liveness probe for the Go integration contract.
//!
//! Like `status`, it uses CDP detection only (no connection is opened). The
//! difference is the exit-code contract: `0` when a browser is running,
//! `1` when it is not. This makes `gthings health` a binary readiness gate
//! that downstream orchestration (e.g. Go) can poll cheaply.

use gthings_cdp::CdpError;

use crate::util::{UniversalFlags, emit_browser_envelope, emit_error};

/// Health: detect only, no connect. Exit 0 if a browser is running, 1 otherwise.
pub(crate) async fn cmd_health(flags: &UniversalFlags) -> i32 {
    let p = flags.effective_cdp_port();

    match gthings_cdp::detect(p).await {
        Ok(browser) => {
            emit_browser_envelope(flags, &browser);
            0
        }
        Err(CdpError::BrowserNotFound { .. }) => {
            emit_error(
                flags,
                gthings_common::taxonomy::ErrorCode::BrowserNotFound,
                "No CDP browser is running",
                "Start Chrome or Dia with --remote-debugging-port=9222",
            );
            1
        }
        Err(e) => {
            emit_error(
                flags,
                gthings_common::taxonomy::ErrorCode::ConnectionFailed,
                &e.to_string(),
                "Check browser debugging port",
            );
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::util::build_envelope;

    /// The healthy path emits an `ok` envelope whose `data` exposes the
    /// `{browser, version, ws_url}` shape the Go side consumes.
    #[test]
    fn running_envelope_exposes_browser_fields() {
        let value = serde_json::json!({
            "browser": "Dia",
            "version": "132.0.0",
            "ws_url": "ws://127.0.0.1:9222/devtools/browser/abc",
        });
        let envelope = build_envelope(Some(&value), None);
        assert_eq!(envelope["status"], "ok");
        assert_eq!(envelope["data"]["browser"], "Dia");
        assert_eq!(envelope["data"]["version"], "132.0.0");
        assert_eq!(
            envelope["data"]["ws_url"],
            "ws://127.0.0.1:9222/devtools/browser/abc"
        );
        assert!(
            envelope["trace_id"].as_str().is_some_and(|t| !t.is_empty()),
            "health ok envelope must carry trace_id"
        );
    }
}
