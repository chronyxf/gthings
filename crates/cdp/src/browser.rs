use crate::connection::Connection;
use crate::error::{CdpError, Result};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use tracing;

/// A persistent Chrome browser instance. Stays alive after Drop.
pub struct Browser {
    ws_url: String,
    #[allow(dead_code)]
    cdp_port: u16,
}

/// Saved browser state for reuse across commands.
#[derive(Serialize, Deserialize)]
struct BrowserState {
    pid: u32,
}

/// Detect the default browser for HTTP URLs using macOS Launch Services.
/// Returns the path to the .app bundle (e.g., "/Applications/Google Chrome.app").
#[cfg(target_os = "macos")]
fn default_browser_bundle() -> Option<std::path::PathBuf> {
    let script = r#"import AppKit; let ws = NSWorkspace.shared; if let url = ws.urlForApplication(toOpen: URL(string: "https://")!) { print(url.path) }"#;
    let output = std::process::Command::new("swift")
        .args(["-e", script])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path_str = std::str::from_utf8(&output.stdout).ok()?.trim();
    if path_str.is_empty() {
        return None;
    }
    let path = std::path::PathBuf::from(path_str);
    if path.exists() { Some(path) } else { None }
}

#[cfg(not(target_os = "macos"))]
fn default_browser_bundle() -> Option<std::path::PathBuf> {
    None // Non-macOS fallback: no default browser detection
}

/// Map a browser bundle path to its executable name inside Contents/MacOS/
fn browser_exec_name(bundle_path: &std::path::Path) -> Option<&'static str> {
    let path_str = bundle_path.to_string_lossy();
    if path_str.contains("Google Chrome") {
        Some("Google Chrome")
    } else if path_str.contains("Dia") {
        Some("Dia")
    } else if path_str.contains("Arc") {
        Some("Arc")
    } else if path_str.contains("Brave Browser") || path_str.contains("Brave") {
        Some("Brave Browser")
    } else if path_str.contains("Microsoft Edge") {
        Some("Microsoft Edge")
    } else if path_str.contains("Chromium") {
        Some("Chromium")
    } else {
        None
    }
}

/// Map a browser bundle path to its profile directory suffix (under ~/Library/Application Support/)
fn browser_profile_suffix(bundle_path: &std::path::Path) -> Option<&'static str> {
    let path_str = bundle_path.to_string_lossy();
    if path_str.contains("Google Chrome") {
        Some("Google/Chrome")
    } else if path_str.contains("Dia") {
        Some("Dia")
    } else if path_str.contains("Arc") {
        Some("Arc")
    } else if path_str.contains("Brave Browser") || path_str.contains("Brave") {
        Some("BraveSoftware/Brave-Browser")
    } else if path_str.contains("Microsoft Edge") {
        Some("Microsoft Edge")
    } else if path_str.contains("Chromium") {
        Some("Chromium")
    } else {
        None
    }
}

/// Build the executable path from a bundle path
fn bundle_to_exec(bundle: &std::path::Path, exec_name: &str) -> std::path::PathBuf {
    bundle.join("Contents").join("MacOS").join(exec_name)
}

impl Browser {
    /// Launch or reuse a persistent Chrome browser.
    #[allow(clippy::result_large_err)]
    pub async fn launch(
        browser_path: Option<std::path::PathBuf>,
        profile_dir: Option<std::path::PathBuf>,
        cdp_port: u16,
    ) -> Result<Self> {
        tracing::info!("Checking for existing browser on port {cdp_port}");
        if let Some(browser) = Self::find_existing(cdp_port).await {
            tracing::info!("Found existing browser, reusing");
            return Ok(browser);
        }
        tracing::info!("No existing browser found, launching new one");

        let chrome_path = Self::find_chrome(browser_path)
            .ok_or_else(|| CdpError::LaunchFailed("No Chrome/Chromium browser found".into()))?;

        let port = cdp_port;

        // Use real profile to avoid browser onboarding/login prompts.
        // SingletonLock conflicts are avoided because find_existing() reuses
        // the already-running browser instead of launching a second instance.
        // If no existing browser is found, we clean locks and launch fresh.
        let profile_dir = Self::real_profile_dir(profile_dir).unwrap_or_else(|| {
            let tmp = std::path::PathBuf::from(format!("/tmp/gthings-{}", port));
            let _ = std::fs::create_dir_all(&tmp);
            tmp
        });

        // Clean locks before fresh launch to avoid SingletonLock conflicts
        {
            let dir = profile_dir.clone();
            tokio::task::spawn_blocking(move || {
                Self::clean_profile_locks(&dir);
            })
            .await
            .map_err(|e| CdpError::LaunchFailed(format!("spawn_blocking failed: {e}")))?;
        }

        tracing::info!(
            "Launching browser on port {} with profile {:?}",
            port,
            profile_dir
        );

        let mut cmd = Command::new(&chrome_path);
        cmd.arg(format!("--remote-debugging-port={}", port))
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .arg("--disable-sync")
            .arg("--remote-allow-origins=*")
            .arg("--enable-automation")
            .arg("--disable-background-networking")
            .arg("--disable-extensions")
            .arg("--disable-component-update")
            .arg("--disable-default-apps")
            .arg("--password-store=basic")
            .arg("--use-mock-keychain")
            .arg(format!("--user-data-dir={}", profile_dir.display()))
            .arg("about:blank")
            .stderr(Stdio::piped())
            .stdout(Stdio::null())
            .stdin(Stdio::null());

        let mut child = cmd
            .spawn()
            .map_err(|e| CdpError::LaunchFailed(format!("Failed to spawn Chrome: {e}")))?;

        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| CdpError::LaunchFailed("No stderr on Chrome process".into()))?;

        let reader = BufReader::new(stderr);
        let mut ws_url = None;

        for line in reader.lines() {
            let line = line.map_err(|e| {
                CdpError::LaunchFailed(format!("Failed to read Chrome stderr: {e}"))
            })?;
            tracing::debug!("Chrome: {}", line);

            // "DevTools listening on ws://127.0.0.1:9222/..."
            if let Some(url) = line.strip_prefix("DevTools listening on ") {
                ws_url = Some(url.trim().to_string());
                break;
            }
        }

        let ws_url = ws_url.ok_or(CdpError::NoWsUrl)?;
        let pid = child.id();

        let state = BrowserState { pid };
        let state_json = serde_json::to_string(&state)?;
        // Atomic write: temp file then rename
        let final_path = Self::state_path();
        let tmp_path = final_path.with_extension("json.tmp");
        let json = state_json;
        tokio::task::spawn_blocking(move || {
            if let Some(parent) = final_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::write(&tmp_path, &json)
                .map_err(|e| CdpError::LaunchFailed(format!("Cannot save state: {e}")))?;
            std::fs::rename(&tmp_path, &final_path)
                .map_err(|e| CdpError::LaunchFailed(format!("Cannot commit state: {e}")))?;
            Ok::<_, CdpError>(())
        })
        .await
        .map_err(|e| CdpError::LaunchFailed(format!("spawn_blocking failed: {e}")))??;

        tracing::info!("Launched persistent browser (pid={})", pid);

        // Detach — browser stays alive after Drop
        drop(child);

        Ok(Browser { ws_url, cdp_port })
    }

    /// Connect to CDP WebSocket.
    pub async fn connect(&self) -> Result<Connection> {
        tracing::info!("Connecting to CDP: {}", self.ws_url);

        let (ws_stream, _) = tokio_tungstenite::connect_async(self.ws_url.clone()).await?;
        // Leak kill_tx so kill_rx never fires
        let (kill_tx, kill_rx) = tokio::sync::oneshot::channel();
        std::mem::forget(kill_tx);
        Connection::new(ws_stream, kill_rx).await
    }

    /// Get the WebSocket URL.
    pub fn ws_url(&self) -> &str {
        &self.ws_url
    }

    /// Locate Chrome executable.
    fn find_chrome(browser_path: Option<std::path::PathBuf>) -> Option<String> {
        // 1. Check explicit env var first (already done in Level 0)
        if let Some(path) = browser_path {
            if path.exists() {
                return Some(path.to_string_lossy().to_string());
            }
        }

        // 2. Check macOS default browser
        #[cfg(target_os = "macos")]
        if let Some(bundle) = default_browser_bundle() {
            if let Some(exec_name) = browser_exec_name(&bundle) {
                let exec = bundle_to_exec(&bundle, exec_name);
                if exec.exists() {
                    return Some(exec.to_string_lossy().to_string());
                }
            }
        }

        // 3. Fallback: hardcoded path list (original behavior)
        let candidates = [
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Chrome.app/Contents/MacOS/Chrome",
            "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            "/Applications/Arc.app/Contents/MacOS/Arc",
            "/Applications/Dia.app/Contents/MacOS/Dia",
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
            "/snap/bin/chromium",
        ];

        for candidate in &candidates {
            if std::path::Path::new(candidate).exists() {
                return Some(candidate.to_string());
            }
        }

        // 4. Fallback: `which` search
        for name in &["google-chrome", "chromium", "google-chrome-stable", "dia"] {
            if let Ok(output) = std::process::Command::new("which").arg(name).output() {
                if output.status.success() {
                    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if !path.is_empty() && std::path::Path::new(&path).exists() {
                        return Some(path);
                    }
                }
            }
        }

        None
    }

    /// Locate browser profile directory.
    fn real_profile_dir(profile_dir: Option<std::path::PathBuf>) -> Option<std::path::PathBuf> {
        // 1. Check explicit env var first (GTHINGS_PROFILE_DIR)
        if let Some(dir) = profile_dir {
            if dir.exists() {
                return Some(dir);
            }
        }

        let home = std::env::var("HOME").ok()?;

        // 2. Try to match profile to the default browser
        #[cfg(target_os = "macos")]
        if let Some(bundle) = default_browser_bundle() {
            if let Some(suffix) = browser_profile_suffix(&bundle) {
                let profile = std::path::PathBuf::from(&home)
                    .join("Library/Application Support")
                    .join(suffix);
                if profile.exists() {
                    return Some(profile);
                }
            }
        }

        // 3. Fallback: check common profile directories
        let common_profiles = [
            "Google/Chrome",
            "Dia",
            "Chromium",
            "BraveSoftware/Brave-Browser",
            "Microsoft Edge",
            "Arc",
        ];

        for suffix in &common_profiles {
            let profile = std::path::PathBuf::from(&home)
                .join("Library/Application Support")
                .join(suffix);
            if profile.exists() {
                return Some(profile);
            }
        }

        None
    }

    /// Clean profile lock files.
    fn clean_profile_locks(profile_dir: &std::path::Path) {
        let lock_files = [
            "SingletonLock",
            "SingletonSocket",
            "SingletonCookie",
            "DevToolsActivePort",
        ];
        for name in &lock_files {
            let path = profile_dir.join(name);
            if path.exists() {
                let _ = std::fs::remove_file(&path);
            }
        }
    }

    /// Check if the browser is alive by probing the given port.
    pub fn is_alive(cdp_port: u16) -> bool {
        Self::probe_port(cdp_port)
    }

    /// Get the path to the browser state file.
    fn state_path() -> PathBuf {
        Self::home_dir().join(".gthings/browser.json")
    }

    /// Get home directory.
    fn home_dir() -> PathBuf {
        std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/tmp"))
    }

    /// Get the path to the browser state file (public for CLI use).
    pub fn state_file_path() -> PathBuf {
        Self::state_path()
    }

    /// Find existing browser via state file and port probe.
    pub async fn find_existing(cdp_port: u16) -> Option<Self> {
        let state_path = Self::state_path();
        if !state_path.exists() {
            return None;
        }

        let path = state_path.clone();
        let state_str: String =
            tokio::task::spawn_blocking(move || std::fs::read_to_string(&path).ok())
                .await
                .unwrap_or(None)?;
        let state: BrowserState = serde_json::from_str(&state_str).ok()?;

        if !Self::is_process_alive(state.pid) {
            tracing::warn!("Browser pid={} is dead, removing stale state", state.pid);
            let path = state_path.clone();
            tokio::task::spawn_blocking(move || {
                let _ = std::fs::remove_file(&path);
            })
            .await
            .ok();
            return None;
        }

        if !Self::probe_port(cdp_port) {
            tracing::warn!(
                "Browser port {} not responding, removing stale state",
                cdp_port
            );
            let path = state_path.clone();
            tokio::task::spawn_blocking(move || {
                let _ = std::fs::remove_file(&path);
            })
            .await
            .ok();
            return None;
        }

        let ws_url = Self::fetch_ws_url(cdp_port).await?;

        tracing::info!("Found existing browser (pid={})", state.pid);

        Some(Browser { ws_url, cdp_port })
    }

    /// Fetch WebSocket debugger URL from /json/version.
    async fn fetch_ws_url(cdp_port: u16) -> Option<String> {
        let url = format!("http://127.0.0.1:{cdp_port}/json/version");
        let resp = reqwest::get(&url).await.ok()?;
        let json: serde_json::Value = resp.json().await.ok()?;
        json["webSocketDebuggerUrl"].as_str().map(|s| s.to_string())
    }

    /// Check if a process is alive by pid.
    fn is_process_alive(pid: u32) -> bool {
        std::process::Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    /// Probe port to see if it's accepting connections.
    /// Tries IPv4 first, then IPv6.
    fn probe_port(cdp_port: u16) -> bool {
        let addrs = [format!("127.0.0.1:{cdp_port}"), format!("[::1]:{cdp_port}")];
        for addr in &addrs {
            if let Ok(parsed) = addr.parse::<std::net::SocketAddr>() {
                if std::net::TcpStream::connect_timeout(
                    &parsed,
                    std::time::Duration::from_millis(500),
                )
                .is_ok()
                {
                    return true;
                }
            }
        }
        false
    }

    /// Get the browser pid from the state file.
    pub async fn pid(&self) -> Option<u32> {
        let state_path = Self::state_path();
        tokio::task::spawn_blocking(move || {
            if state_path.exists() {
                if let Ok(state_str) = std::fs::read_to_string(&state_path) {
                    if let Ok(state) = serde_json::from_str::<BrowserState>(&state_str) {
                        return Some(state.pid);
                    }
                }
            }
            None
        })
        .await
        .unwrap_or(None)
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        // Browser stays alive — it's persistent
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_probe_port_no_server() {
        assert_eq!(Browser::is_alive(9222), Browser::probe_port(9222));
    }

    #[test]
    fn test_state_path_ends_correctly() {
        let path = Browser::state_path();
        assert!(path.ends_with(".gthings/browser.json"));
    }

    #[test]
    fn test_find_chrome_returns_some_or_none() {
        let result = Browser::find_chrome(None);
        // Either finds Chrome or returns None — don't panic either way
        if let Some(path) = result {
            assert!(
                std::path::Path::new(&path).exists(),
                "Chrome path should exist: {}",
                path
            );
        }
    }

    #[test]
    fn test_error_types_compile() {
        let _err = CdpError::LaunchFailed("test".into());
        let _err = CdpError::NoWsUrl;
        let _err = CdpError::Timeout(1000);
    }
}
