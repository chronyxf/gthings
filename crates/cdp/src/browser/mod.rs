use crate::error::{CdpError, Result};

mod active_port;
mod dialog;

pub(crate) use active_port::probe_devtools_active_port;
#[cfg(target_os = "macos")]
pub(crate) use dialog::dismiss_allow_debugging_dialog;

/// Environment variable that bypasses detection with a direct WebSocket URL.
const ENV_CDP_WS_URL: &str = "GTHINGS_CDP_WS_URL";

/// Resolve the CDP WebSocket URL from the `GTHINGS_CDP_WS_URL` environment
/// variable, returning `None` when unset or empty.
pub(crate) fn ws_url_from_env() -> Option<String> {
    match std::env::var(ENV_CDP_WS_URL) {
        Ok(url) if !url.is_empty() => Some(url),
        _ => None,
    }
}

/// Info about a running browser discovered by [`detect`].
#[derive(Debug, Clone, serde::Serialize)]
pub struct DetectedBrowser {
    pub ws_url: String,
    pub browser: String,
    pub version: String,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Multi-strategy browser detection.
///
/// Tries each strategy in order and returns the first successful result:
///
/// 1. **`GTHINGS_CDP_WS_URL`** environment variable (fastest bypass).
/// 2. **HTTP GET** `http://{GTHINGS_CDP_HOST}:{port}/json/version` — parses `webSocketDebuggerUrl`.
/// 3. **HTTP GET** `http://{GTHINGS_CDP_HOST}:{port}/json` — finds first `webSocketDebuggerUrl`.
/// 4. **HTTP GET** `http://{GTHINGS_CDP_HOST}:{port}/json/list` — finds first `webSocketDebuggerUrl`.
/// 5. **DevToolsActivePort** scan across 10+ browser profile directories on macOS.
///
/// The host used by all probe strategies is resolved from the
/// `GTHINGS_CDP_HOST` environment variable (default `127.0.0.1`), so a remote
/// debugging target can be reached without changing code.
///
/// Returns [`CdpError::BrowserNotFound`] if all strategies fail.
pub async fn detect(port: u16) -> Result<DetectedBrowser> {
    // 1. Environment variable bypass
    if let Some(ws_url) = ws_url_from_env() {
        tracing::info!("detect: using {ENV_CDP_WS_URL} env var");
        return Ok(DetectedBrowser {
            ws_url,
            browser: "env".into(),
            version: "unknown".into(),
        });
    }

    // 2. HTTP /json/version (daemon-side probe with Host: localhost header)
    if let Some(browser) = crate::discovery::probe_version(port).await {
        tracing::info!("detect: found via /json/version");
        return Ok(browser);
    }

    // 3–4. HTTP /json and /json/list
    for path in ["/json", "/json/list"] {
        if let Some(ws_url) = crate::discovery::probe_list(port, path).await {
            tracing::info!("detect: found via {path}");
            return Ok(DetectedBrowser {
                ws_url,
                browser: "unknown".into(),
                version: "unknown".into(),
            });
        }
    }

    // 5. DevToolsActivePort scan
    if let Some(browser) = probe_devtools_active_port(port).await {
        tracing::info!("detect: found via DevToolsActivePort");
        return Ok(browser);
    }

    Err(CdpError::BrowserNotFound { port })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_no_browser() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(async { detect(29_999).await });
        assert!(result.is_err(), "detect on unused port should return error");
        match result {
            Err(CdpError::BrowserNotFound { port }) => assert_eq!(port, 29_999),
            _ => panic!("expected BrowserNotFound"),
        }
    }

    #[test]
    fn test_detected_browser_serialize() {
        let db = DetectedBrowser {
            ws_url: "ws://127.0.0.1:9222/devtools/browser/abc".into(),
            browser: "Chrome".into(),
            version: "130.0.0.0".into(),
        };
        let json_str = serde_json::to_string(&db).unwrap();
        // Round-trip through Value to assert specific field values
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(
            parsed["ws_url"].as_str(),
            Some("ws://127.0.0.1:9222/devtools/browser/abc")
        );
        assert_eq!(parsed["browser"].as_str(), Some("Chrome"));
        assert_eq!(parsed["version"].as_str(), Some("130.0.0.0"));
    }

    #[test]
    fn test_cdp_host_resolution_default_and_override() {
        use crate::discovery::cdp_host_from;
        // Override: env var set to a non-empty value → wins.
        assert_eq!(cdp_host_from(Some("10.0.0.7")), "10.0.0.7");
        // Empty string → default.
        assert_eq!(cdp_host_from(Some("")), "127.0.0.1");
        // Unset → default.
        assert_eq!(cdp_host_from(None), "127.0.0.1");
    }

    #[test]
    fn test_probe_urls_use_configured_host() {
        use crate::discovery::{cdp_socket_addr, http_probe_url, ws_probe_url};
        // All probe sites embed the configured (override) CDP host.
        assert_eq!(
            http_probe_url("10.0.0.7", 9222, "/json/version"),
            "http://10.0.0.7:9222/json/version"
        );
        assert_eq!(
            http_probe_url("10.0.0.7", 9222, "/json/list"),
            "http://10.0.0.7:9222/json/list"
        );
        assert_eq!(
            ws_probe_url("10.0.0.7", 9222, "/devtools/browser/abc"),
            "ws://10.0.0.7:9222/devtools/browser/abc"
        );
        assert_eq!(
            cdp_socket_addr("10.0.0.7", 9222),
            Some("10.0.0.7:9222".parse().unwrap())
        );

        // Default host (127.0.0.1) is used when the env var is unset/empty.
        assert_eq!(
            http_probe_url("127.0.0.1", 9222, "/json/version"),
            "http://127.0.0.1:9222/json/version"
        );
        assert_eq!(
            ws_probe_url("127.0.0.1", 9222, "/devtools/browser/abc"),
            "ws://127.0.0.1:9222/devtools/browser/abc"
        );
        assert_eq!(
            cdp_socket_addr("127.0.0.1", 9222),
            Some("127.0.0.1:9222".parse().unwrap())
        );
    }
}
