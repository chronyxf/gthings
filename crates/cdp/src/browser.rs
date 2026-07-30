use crate::connection::Connection;
use crate::error::{CdpError, Result};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

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
    Connection::connect(ws_url, None).await
}

/// Dismiss the macOS "Allow remote debugging connection?" dialog that appears
/// as a **sheet** in Dia and other Chromium-based browsers when a CDP
/// connection is first attempted.
///
/// Uses `osascript`/System Events to detect the dialog sheet and click the
/// "Allow" button. Polls every 500 ms for up to ~10 seconds (20 attempts)
/// since the dialog may take 1–3 seconds to appear. Logs a warning if the
/// dialog is never found — the WebSocket handshake may still proceed.
#[cfg(target_os = "macos")]
pub async fn dismiss_allow_debugging_dialog() {
    /// AppleScript that checks each known browser process for a sheet dialog
    /// (attached to window 1) and clicks the "Allow" button if found.
    /// Returns the browser name if dismissed, or empty string otherwise.
    /// Each `exists` check is wrapped in `try` so that missing processes
    /// don't abort the script.
    const SCRIPT: &str = r#"tell application "System Events"
        set browserName to ""
        try
            if exists (sheet 1 of window 1 of process "Dia") then
                tell process "Dia" to click button "Allow" of sheet 1 of window 1
                set browserName to "Dia"
            end if
        end try
        try
            if browserName is "" and exists (sheet 1 of window 1 of process "Chromium") then
                tell process "Chromium" to click button "Allow" of sheet 1 of window 1
                set browserName to "Chromium"
            end if
        end try
        try
            if browserName is "" and exists (sheet 1 of window 1 of process "Google Chrome") then
                tell process "Google Chrome" to click button "Allow" of sheet 1 of window 1
                set browserName to "Google Chrome"
            end if
        end try
        try
            if browserName is "" and exists (sheet 1 of window 1 of process "Google Chrome Canary") then
                tell process "Google Chrome Canary" to click button "Allow" of sheet 1 of window 1
                set browserName to "Google Chrome Canary"
            end if
        end try
        try
            if browserName is "" and exists (sheet 1 of window 1 of process "Microsoft Edge") then
                tell process "Microsoft Edge" to click button "Allow" of sheet 1 of window 1
                set browserName to "Microsoft Edge"
            end if
        end try
        try
            if browserName is "" and exists (sheet 1 of window 1 of process "Microsoft Edge Canary") then
                tell process "Microsoft Edge Canary" to click button "Allow" of sheet 1 of window 1
                set browserName to "Microsoft Edge Canary"
            end if
        end try
        try
            if browserName is "" and exists (sheet 1 of window 1 of process "Brave Browser") then
                tell process "Brave Browser" to click button "Allow" of sheet 1 of window 1
                set browserName to "Brave Browser"
            end if
        end try
        try
            if browserName is "" and exists (sheet 1 of window 1 of process "Arc") then
                tell process "Arc" to click button "Allow" of sheet 1 of window 1
                set browserName to "Arc"
            end if
        end try
        try
            if browserName is "" and exists (sheet 1 of window 1 of process "Vivaldi") then
                tell process "Vivaldi" to click button "Allow" of sheet 1 of window 1
                set browserName to "Vivaldi"
            end if
        end try
        try
            if browserName is "" and exists (sheet 1 of window 1 of process "Opera") then
                tell process "Opera" to click button "Allow" of sheet 1 of window 1
                set browserName to "Opera"
            end if
        end try
        return browserName
    end tell"#;

    const OSASCRIPT_TIMEOUT: Duration = Duration::from_secs(1);
    const MAX_ATTEMPTS: u32 = 1;

    for attempt in 1..=MAX_ATTEMPTS {
        let script = SCRIPT.to_owned();

        let result = tokio::time::timeout(OSASCRIPT_TIMEOUT, async {
            tokio::process::Command::new("osascript")
                .args(["-e", &script])
                .output()
                .await
        })
        .await;

        match result {
            Ok(Ok(out)) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let browser = stdout.trim();
                if !browser.is_empty() {
                    tracing::warn!(
                        "Dismissed remote debugging dialog for {browser} (attempt {attempt})"
                    );
                    return;
                }
            }
            Ok(Err(e)) => {
                tracing::warn!("osascript command failed: {e}");
            }
            Err(_) => {
                tracing::warn!(
                    "osascript timed out after {OSASCRIPT_TIMEOUT:?} (attempt {attempt})"
                );
                // Timeout dropped the future, which drops the Child process,
                // killing the osascript process. Continue to next attempt.
            }
        }

        if attempt < MAX_ATTEMPTS {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    tracing::warn!("Remote debugging dialog not found after {MAX_ATTEMPTS} attempts — continuing");
}

/// Non-macOS: no-op.
#[cfg(not(target_os = "macos"))]
pub async fn dismiss_allow_debugging_dialog() {}
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
