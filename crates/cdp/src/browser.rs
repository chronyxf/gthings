use crate::connection::Connection;
use crate::error::{CdpError, Result};
use std::path::PathBuf;
use std::sync::OnceLock;

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
/// 2. **HTTP GET** `http://127.0.0.1:{port}/json/version` — parses `webSocketDebuggerUrl`.
/// 3. **HTTP GET** `http://127.0.0.1:{port}/json` — finds first `webSocketDebuggerUrl`.
/// 4. **HTTP GET** `http://127.0.0.1:{port}/json/list` — finds first `webSocketDebuggerUrl`.
/// 5. **DevToolsActivePort** scan across 10+ browser profile directories on macOS.
///
/// Returns [`CdpError::BrowserNotFound`] if all strategies fail.
pub async fn detect(port: u16) -> Result<DetectedBrowser> {
    // 1. Environment variable bypass
    if let Ok(ws_url) = std::env::var("GTHINGS_CDP_WS_URL") {
        if !ws_url.is_empty() {
            tracing::info!("detect: using GTHINGS_CDP_WS_URL env var");
            return Ok(DetectedBrowser {
                ws_url,
                browser: "env".into(),
                version: "unknown".into(),
            });
        }
    }

    // 2. HTTP /json/version
    if let Some(browser) = probe_http_version(port).await {
        tracing::info!("detect: found via /json/version");
        return Ok(browser);
    }

    // 3. HTTP /json
    if let Some(ws_url) = probe_http_list(port, "/json").await {
        tracing::info!("detect: found via /json");
        return Ok(DetectedBrowser {
            ws_url,
            browser: "unknown".into(),
            version: "unknown".into(),
        });
    }

    // 4. HTTP /json/list
    if let Some(ws_url) = probe_http_list(port, "/json/list").await {
        tracing::info!("detect: found via /json/list");
        return Ok(DetectedBrowser {
            ws_url,
            browser: "unknown".into(),
            version: "unknown".into(),
        });
    }

    // 5. DevToolsActivePort scan
    if let Some(browser) = probe_devtools_active_port(port).await {
        tracing::info!("detect: found via DevToolsActivePort");
        return Ok(browser);
    }

    Err(CdpError::BrowserNotFound { port })
}

/// Connect to a browser's CDP WebSocket endpoint.
///
/// This is a thin wrapper around [`Connection::connect`].
pub async fn connect(ws_url: &str) -> Result<Connection> {
    Connection::connect(ws_url).await
}

/// Dismiss the macOS "Allow remote debugging connection?" dialog that Dia
/// shows when a CDP connection is first attempted.
///
/// Sends a Return keystroke to the Dia process via `osascript`/System Events.
#[cfg(target_os = "macos")]
pub fn dismiss_allow_debugging_dialog() {
    let script = r#"tell application "System Events"
        try
            set frontmost of process "Dia" to true
        end try
        tell process "Dia" to keystroke return
    end tell"#;
    let _ = std::process::Command::new("osascript")
        .args(["-e", script])
        .output();
}

/// Non-macOS: no-op.
#[cfg(not(target_os = "macos"))]
pub fn dismiss_allow_debugging_dialog() {}
// ---------------------------------------------------------------------------
// Internal probe helpers
// ---------------------------------------------------------------------------

/// Shared HTTP client with sensible timeouts.
fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(3))
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("valid reqwest client config")
    })
}

/// Probe `/json/version` — the richest endpoint (includes `webSocketDebuggerUrl`,
/// `Browser`, and version info).
async fn probe_http_version(port: u16) -> Option<DetectedBrowser> {
    let url = format!("http://127.0.0.1:{port}/json/version");
    let client = http_client();

    let resp = client.get(&url).send().await.ok()?;
    let body: serde_json::Value = resp.json().await.ok()?;

    let ws_url = body.get("webSocketDebuggerUrl")?.as_str()?;
    let full_browser = body
        .get("Browser")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let version = full_browser.to_string();
    // Extract short browser name from "Chrome/130.0.0.0" style string
    let browser = full_browser
        .split('/')
        .next()
        .unwrap_or("unknown")
        .to_string();

    Some(DetectedBrowser {
        ws_url: ws_url.to_string(),
        browser,
        version,
    })
}

/// Probe `/json` or `/json/list` — returns the `webSocketDebuggerUrl` of
/// the first available page target.
async fn probe_http_list(port: u16, path: &str) -> Option<String> {
    let url = format!("http://127.0.0.1:{port}{path}");
    let client = http_client();

    let resp = client.get(&url).send().await.ok()?;
    let list: Vec<serde_json::Value> = resp.json().await.ok()?;

    for entry in &list {
        if let Some(ws_url) = entry.get("webSocketDebuggerUrl").and_then(|v| v.as_str()) {
            return Some(ws_url.to_string());
        }
    }

    None
}

/// Scan well-known browser profile directories for a `DevToolsActivePort`
/// file whose port matches the requested port, then verify via TCP connect.
async fn probe_devtools_active_port(port: u16) -> Option<DetectedBrowser> {
    let profile_dirs = get_profile_dirs();
    if profile_dirs.is_empty() {
        return None;
    }

    // Synchronous file reads are fine here — negligible for a handful of files.
    let result: Option<DetectedBrowser> = tokio::task::spawn_blocking(move || {
        for dir in &profile_dirs {
            let active_port_path = dir.join("DevToolsActivePort");
            let content = match std::fs::read_to_string(&active_port_path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let lines: Vec<&str> = content.trim().lines().collect();
            if lines.len() < 2 {
                continue;
            }
            let file_port: u16 = match lines[0].trim().parse().ok() {
                Some(p) => p,
                None => continue,
            };
            if file_port != port {
                continue;
            }
            let ws_path = lines[1].trim();
            let ws_url = format!("ws://127.0.0.1:{port}{ws_path}");

            // Verify port is accepting TCP connections
            let addr: std::net::SocketAddr = format!("127.0.0.1:{port}").parse().ok()?;
            if std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(500))
                .is_ok()
            {
                let browser_name = infer_browser_name(dir);
                return Some(DetectedBrowser {
                    ws_url,
                    browser: browser_name,
                    version: "unknown".into(),
                });
            }
        }
        None
    })
    .await
    .ok()?;
    result
}

/// Return all possible macOS browser profile directories.
fn get_profile_dirs() -> Vec<PathBuf> {
    let home = std::env::var("HOME").ok().map(PathBuf::from);
    let home = match home {
        Some(h) => h,
        None => return Vec::new(),
    };

    let app_support = home.join("Library/Application Support");
    let dirs = vec![
        app_support.join("Dia/User Data"),
        app_support.join("Google/Chrome"),
        app_support.join("Google/Chrome Canary"),
        app_support.join("Chromium"),
        app_support.join("Microsoft Edge"),
        app_support.join("Microsoft Edge Canary"),
        app_support.join("BraveSoftware/Brave-Browser"),
        app_support.join("Arc/User Data"),
        app_support.join("Vivaldi"),
        app_support.join("com.operasoftware.Opera"),
    ];
    dirs.into_iter().filter(|p| p.exists()).collect()
}

/// Infer a human-readable browser name from a profile directory path.
fn infer_browser_name(path: &std::path::Path) -> String {
    let s = path.to_string_lossy();
    let s = s.as_ref();
    if s.contains("Dia") {
        "Dia".into()
    } else if s.contains("Chrome Canary") {
        "Google Chrome Canary".into()
    } else if s.contains("Chrome") {
        "Google Chrome".into()
    } else if s.contains("Chromium") {
        "Chromium".into()
    } else if s.contains("Edge Canary") {
        "Microsoft Edge Canary".into()
    } else if s.contains("Edge") {
        "Microsoft Edge".into()
    } else if s.contains("Brave") {
        "Brave".into()
    } else if s.contains("Arc") {
        "Arc".into()
    } else if s.contains("Vivaldi") {
        "Vivaldi".into()
    } else if s.contains("Opera") {
        "Opera".into()
    } else {
        "unknown".into()
    }
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
    fn test_infer_browser_name() {
        fn p(s: &str) -> &std::path::Path {
            std::path::Path::new(s)
        }
        assert_eq!(infer_browser_name(p("/Dia/User Data")), "Dia");
        assert_eq!(infer_browser_name(p("/Google/Chrome")), "Google Chrome");
        assert_eq!(
            infer_browser_name(p("/Google/Chrome Canary")),
            "Google Chrome Canary"
        );
        assert_eq!(infer_browser_name(p("/Chromium")), "Chromium");
        assert_eq!(infer_browser_name(p("/Microsoft Edge")), "Microsoft Edge");
        assert_eq!(
            infer_browser_name(p("/BraveSoftware/Brave-Browser")),
            "Brave"
        );
        assert_eq!(infer_browser_name(p("/Arc/User Data")), "Arc");
        assert_eq!(infer_browser_name(p("/Vivaldi")), "Vivaldi");
        assert_eq!(infer_browser_name(p("/com.operasoftware.Opera")), "Opera");
        assert_eq!(infer_browser_name(p("/Unknown/Path")), "unknown");
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
}
