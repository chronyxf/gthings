use crate::connection::Connection;
use crate::error::{CdpError, Result};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use tracing;

const CDP_PORT: u16 = 9222;

/// A persistent Chrome browser instance. Stays alive after Drop.
pub struct Browser {
    ws_url: String,
}

/// Saved browser state for reuse across commands.
#[derive(Serialize, Deserialize)]
struct BrowserState {
    pid: u32,
}

impl Browser {
    /// Launch or reuse a persistent Chrome browser on port 9222.
    #[allow(clippy::result_large_err)]
    pub async fn launch() -> Result<Self> {
        tracing::info!("Checking for existing browser on port 9222");
        if let Some(browser) = Self::find_existing().await {
            tracing::info!("Found existing browser, reusing");
            return Ok(browser);
        }
        tracing::info!("No existing browser found, launching new one");

        let chrome_path = Self::find_chrome()
            .ok_or_else(|| CdpError::LaunchFailed("No Chrome/Chromium browser found".into()))?;

        let port = CDP_PORT;

        // Use real profile to avoid onboarding / first-run dialogs
        let profile_dir = Self::real_profile_dir()
            .unwrap_or_else(|| std::path::PathBuf::from(format!("/tmp/cdp-profile-{}", port)));
        {
            let dir = profile_dir.clone();
            tokio::task::spawn_blocking(move || {
                Self::clean_profile_locks(&dir);
            })
            .await
            .map_err(|e| CdpError::LaunchFailed(format!("spawn_blocking failed: {e}")))?;
        }

        tracing::info!(
            "Launching Chrome on port {} with profile {:?}",
            port,
            profile_dir
        );

        let mut cmd = Command::new(&chrome_path);
        cmd.arg(format!("--remote-debugging-port={}", port))
            .arg("--no-first-run")
            .arg("--remote-allow-origins=*")
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

        Ok(Browser { ws_url })
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
    fn find_chrome() -> Option<String> {
        let candidates = [
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Chrome.app/Contents/MacOS/Chrome",
            "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
            "/snap/bin/chromium",
            "/Applications/Dia.app/Contents/MacOS/Dia",
        ];

        for path in &candidates {
            if std::path::Path::new(path).exists() {
                return Some(path.to_string());
            }
        }

        if let Ok(path) = std::process::Command::new("which")
            .arg("google-chrome")
            .arg("chromium")
            .arg("google-chrome-stable")
            .arg("dia")
            .output()
        {
            let output = String::from_utf8_lossy(&path.stdout);
            for line in output.lines() {
                if !line.is_empty() {
                    return Some(line.to_string());
                }
            }
        }

        None
    }

    /// Locate browser profile directory.
    fn real_profile_dir() -> Option<std::path::PathBuf> {
        let home = std::env::var("HOME").ok()?;
        let candidates = [
            std::path::PathBuf::from(&home).join("Library/Application Support/Dia/User Data"),
            std::path::PathBuf::from(&home).join("Library/Application Support/Google/Chrome"),
            std::path::PathBuf::from(&home).join("Library/Application Support/Chromium"),
        ];

        for candidate in &candidates {
            if candidate.exists() {
                return Some(candidate.clone());
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

    /// Check if the browser is alive by probing port 9222.
    pub fn is_alive() -> bool {
        Self::probe_port()
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
    pub async fn find_existing() -> Option<Self> {
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

        if !Self::probe_port() {
            tracing::warn!(
                "Browser port {} not responding, removing stale state",
                CDP_PORT
            );
            let path = state_path.clone();
            tokio::task::spawn_blocking(move || {
                let _ = std::fs::remove_file(&path);
            })
            .await
            .ok();
            return None;
        }

        let ws_url = Self::fetch_ws_url().await?;

        tracing::info!("Found existing browser (pid={})", state.pid);

        Some(Browser { ws_url })
    }

    /// Fetch WebSocket debugger URL from /json/version.
    async fn fetch_ws_url() -> Option<String> {
        let url = format!("http://127.0.0.1:{}/json/version", CDP_PORT);
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

    /// Probe port 9222 to see if it's accepting connections.
    /// Tries IPv4 first, then IPv6.
    fn probe_port() -> bool {
        let addrs = [
            format!("127.0.0.1:{}", CDP_PORT),
            format!("[::1]:{}", CDP_PORT),
        ];
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
        assert_eq!(Browser::is_alive(), Browser::probe_port());
    }

    #[test]
    fn test_state_path_ends_correctly() {
        let path = Browser::state_path();
        assert!(path.ends_with(".gthings/browser.json"));
    }

    #[test]
    fn test_find_chrome_returns_some_or_none() {
        let result = Browser::find_chrome();
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
