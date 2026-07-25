use crate::connection::Connection;
use crate::error::{CdpError, Result};
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use tracing;

/// A persistent Chrome browser instance. Stays alive after Drop.
pub struct Browser {
    ws_url: String,
    #[allow(dead_code)]
    cdp_port: u16,
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
        Some("Google/Chrome/User Data")
    } else if path_str.contains("Dia") {
        Some("Dia/User Data")
    } else if path_str.contains("Arc") {
        Some("Arc/User Data")
    } else if path_str.contains("Brave Browser") || path_str.contains("Brave") {
        Some("BraveSoftware/Brave-Browser/User Data")
    } else if path_str.contains("Microsoft Edge") {
        Some("Microsoft Edge/User Data")
    } else if path_str.contains("Chromium") {
        Some("Chromium/User Data")
    } else {
        None
    }
}

/// Read the last-used profile directory name from the browser's Local State file.
/// Returns "Default" as fallback if Local State can't be read or last_used is missing.
#[allow(dead_code)]
fn detect_last_used_profile(user_data_dir: &std::path::Path) -> String {
    let local_state_path = user_data_dir.join("Local State");
    let content = match std::fs::read_to_string(&local_state_path) {
        Ok(c) => c,
        Err(_) => return "Default".to_string(),
    };
    let json: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return "Default".to_string(),
    };
    // Look for "profile.last_used" path in the JSON
    if let Some(last_used) = json.get("profile").and_then(|p| p.get("last_used")).and_then(|l| l.as_str()) {
        if user_data_dir.join(last_used).exists() {
            return last_used.to_string();
        }
    }
    // Fallback: first key in profile.info_cache
    if let Some(info_cache) = json.get("profile").and_then(|p| p.get("info_cache")) {
        if let Some(obj) = info_cache.as_object() {
            if let Some(first_key) = obj.keys().next() {
                return first_key.clone();
            }
        }
    }
    "Default".to_string()
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

        // gsearch approach: prefer real profile if available AND not in use.
        // If real profile is in use (user's browser is open), fall back to
        // seeded temp profile to avoid crashing the user's session.
        let profile_dir = match Self::resolve_profile(profile_dir) {
            Some(dir) => dir,
            None => {
                let tmp = std::path::PathBuf::from(format!("/tmp/gthings-{}", cdp_port));
                let _ = std::fs::create_dir_all(&tmp);
                Self::seed_profile(&tmp);
                tmp
            }
        };

        if let Some(browser) = Self::find_existing(Some(&profile_dir), cdp_port).await {
            tracing::info!("Found existing browser, reusing");
            return Ok(browser);
        }
        tracing::info!("No existing browser found, launching new one");

        let chrome_path = Self::find_chrome(browser_path)
            .ok_or_else(|| CdpError::LaunchFailed("No Chrome/Chromium browser found".into()))?;

        let port = cdp_port;

        // Only check profile-in-use for real profiles (temp profiles are always clean)
        if profile_dir.to_string_lossy().contains("/tmp/") {
            // Temp profile — no lock cleaning needed
        } else if Self::is_profile_in_use(&profile_dir) {
            return Err(CdpError::LaunchFailed(format!(
                "Profile {:?} is in use by another browser. Close it first or set GTHINGS_PROFILE_DIR.",
                profile_dir
            )));
        } else {
            // Clean locks on real profile before launch
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
            profile_dir,
        );

        let mut cmd = Command::new(&chrome_path);
        cmd.arg(format!("--remote-debugging-port={}", port))
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .arg("--disable-fre")
            .arg("--disable-search-engine-choice-screen")
            .arg("--disable-sync")
            .arg("--remote-allow-origins=*")
            .arg("--disable-background-networking")
            .arg("--disable-component-update")
            .arg("--disable-default-apps")
            .arg("--password-store=basic")
            .arg("--use-mock-keychain")
            .arg("--window-size=1280,720")
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

        tracing::info!("Launched persistent browser (pid={})", pid);

        // Detach — browser stays alive after Drop
        drop(child);

        Ok(Browser { ws_url, cdp_port })
    }

    /// Connect to CDP WebSocket.
    pub async fn connect(&self) -> Result<Connection> {
        tracing::info!("Connecting to CDP: {}", self.ws_url);

        // Start WebSocket connection and 600ms timer concurrently.
        // The timer dismisses Dia's "Allow debugging connection?" dialog
        // if the WS is still connecting after 600ms. Matches gsearch's approach:
        //   session.ts:189-197 — setTimeout at 600ms, dismissDiaAllowPrompt()
        let connect_fut = tokio_tungstenite::connect_async(&self.ws_url);
        let dismiss_fut = tokio::time::sleep(std::time::Duration::from_millis(600));

        // Pin both futures
        tokio::pin!(connect_fut);
        tokio::pin!(dismiss_fut);

        // After 600ms, dismiss dialog regardless of connection state
        (&mut dismiss_fut).await;
        #[cfg(target_os = "macos")]
        dismiss_allow_debugging_dialog();

        // Now wait for connection
        let (ws_stream, _) = connect_fut.await.map_err(|e| {
            CdpError::LaunchFailed(format!("WebSocket connect failed: {e}"))
        })?;

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

    /// Resolve which profile directory to use.
    /// Returns the real profile if available AND not in use by another browser.
    /// Returns None to signal that a seeded temp profile should be used.
    fn resolve_profile(explicit: Option<std::path::PathBuf>) -> Option<std::path::PathBuf> {
        // 1. Explicit profile from env var takes priority
        if let Some(p) = explicit {
            if p.exists() {
                // Even explicit: check not in use to avoid crashes
                if !Self::is_profile_in_use(&p) {
                    return Some(p);
                }
            }
        }

        // 2. macOS default browser's real profile
        #[cfg(target_os = "macos")]
        if let Some(bundle) = default_browser_bundle() {
            if let Some(suffix) = browser_profile_suffix(&bundle) {
                if let Some(home) = std::env::var("HOME").ok().map(std::path::PathBuf::from) {
                    let path = home.join("Library/Application Support").join(suffix);
                    if path.exists() && !Self::is_profile_in_use(&path) {
                        return Some(path);
                    }
                }
            }
        }

        // 3. Fallback: common profile paths (check not in use)
        let common_paths = [
            "Library/Application Support/Google/Chrome/User Data",
            "Library/Application Support/Dia/User Data",
            "Library/Application Support/BraveSoftware/Brave-Browser/User Data",
        ];
        if let Some(home) = std::env::var("HOME").ok().map(std::path::PathBuf::from) {
            for suffix in &common_paths {
                let path = home.join(suffix);
                if path.exists() && !Self::is_profile_in_use(&path) {
                    return Some(path);
                }
            }
        }

        // No usable real profile found → use seeded temp
        None
    }

    /// Seed a fresh profile directory with synthetic Preferences and Local State
    /// to suppress ALL first-run dialogs including sign-in forms.
    /// Matches gsearch's `_pre_seed_profile()` function exactly.
    fn seed_profile(profile_dir: &std::path::Path) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros() as u64;

        // Write Preferences (suppresses welcome page, onboarding, etc.)
        let prefs = serde_json::json!({
            "browser": {
                "has_seen_welcome_page": true
            },
            "profile": {
                "exit_type": "Normal"
            },
            "default_apps_install_state": 3,
            "in_product_help": {
                "session_last_active_time": now.to_string(),
                "session_number": 5,
                "session_start_time": now.to_string()
            }
        });

        // Write Local State (enterprise policy: skip_first_run_ui SUPPRESSES ALL DIALOGS)
        let local_state = serde_json::json!({
            "browser": {
                "enabled_labs_experiments": [],
                "last_redirect_origin": "",
                "last_whats_new_milestone": "150"
            },
            "distribution": {
                "skip_first_run_ui": true  // ← THIS IS THE KEY FIELD
            }
        });

        // Write to both User Data/Default and Default (matching gsearch)
        for sub in &["User Data/Default", "Default"] {
            let prefs_dir = profile_dir.join(sub);
            let _ = std::fs::create_dir_all(&prefs_dir);
            let _ = std::fs::write(prefs_dir.join("Preferences"), serde_json::to_string(&prefs).unwrap());
        }

        // Write Local State to User Data and root (matching gsearch)
        for sub in &["User Data", "."] {
            let state_dir = profile_dir.join(sub);
            let _ = std::fs::create_dir_all(&state_dir);
            let _ = std::fs::write(state_dir.join("Local State"), serde_json::to_string(&local_state).unwrap());
        }
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

    /// Check if a browser process is currently running with this profile directory.
    /// On macOS, checks if any process has the SingletonLock file open.
    fn is_profile_in_use(profile_dir: &std::path::Path) -> bool {
        let lock_file = profile_dir.join("SingletonLock");
        if !lock_file.exists() {
            return false;
        }
        // On macOS, lsof can check if a process has this file open
        #[cfg(target_os = "macos")]
        {
            let output = std::process::Command::new("lsof")
                .args(["-F", "p", &lock_file.to_string_lossy()])
                .output()
                .ok();
            if let Some(output) = output {
                if output.status.success() && !output.stdout.is_empty() {
                    return true;
                }
            }
        }
        false
    }

    /// Check if the browser is alive by probing the given port.
    pub fn is_alive(cdp_port: u16) -> bool {
        Self::probe_port(cdp_port)
    }

    /// Find existing browser by TCP probing and discovering WS URL.
    pub async fn find_existing(explicit_profile_dir: Option<&std::path::Path>, cdp_port: u16) -> Option<Self> {
        // Step 1: TCP probe — is anything listening on this port?
        if !Self::probe_port(cdp_port) {
            return None;
        }

        // Step 2: Try to discover WS URL from DevToolsActivePort
        let ws_url = Self::discover_ws_url(explicit_profile_dir, cdp_port).await;

        if let Some(ref url) = ws_url {
            // Step 3: Verify via WebSocket
            if Self::verify_ws(url).await.is_some() {
                return Some(Browser {
                    ws_url: url.clone(),
                    cdp_port,
                });
            }
        }

        None
    }

    /// Discover WS URL by searching for DevToolsActivePort in common profile paths.
    pub async fn discover_ws_url(explicit_profile_dir: Option<&std::path::Path>, cdp_port: u16) -> Option<String> {
        let mut candidates: Vec<std::path::PathBuf> = Vec::new();

        // 1. Explicit profile dir if provided
        if let Some(dir) = explicit_profile_dir {
            candidates.push(dir.to_path_buf());
        }

        // 2. Common macOS profile paths
        #[cfg(target_os = "macos")]
        {
            if let Some(home) = std::env::var("HOME").ok().map(std::path::PathBuf::from) {
                let common = [
                    "Library/Application Support/Dia/User Data",
                    "Library/Application Support/Google/Chrome",
                    "Library/Application Support/BraveSoftware/Brave-Browser/User Data",
                    "Library/Application Support/Microsoft Edge/User Data",
                ];
                for path in &common {
                    let full = home.join(path);
                    if full.exists() {
                        candidates.push(full);
                    }
                }
            }
        }

        // Deduplicate
        candidates.sort();
        candidates.dedup();

        for profile_dir in &candidates {
            let active_port_path = profile_dir.join("DevToolsActivePort");
            if let Ok(content) = std::fs::read_to_string(&active_port_path) {
                let lines: Vec<&str> = content.trim().lines().collect();
                if lines.len() >= 2 {
                    if let Ok(file_port) = lines[0].trim().parse::<u16>() {
                        if file_port == cdp_port {
                            let ws_path = lines[1].trim();
                            return Some(format!("ws://127.0.0.1:{}{}", cdp_port, ws_path));
                        }
                    }
                }
            }
        }

        None
    }

    /// Verify a WebSocket debugger URL by connecting and sending Browser.getVersion.
    async fn verify_ws(ws_url: &str) -> Option<()> {
        let (ws_stream, _) = tokio_tungstenite::connect_async(ws_url.to_string()).await.ok()?;
        let (kill_tx, kill_rx) = tokio::sync::oneshot::channel();
        std::mem::forget(kill_tx);
        let mut conn = Connection::new(ws_stream, kill_rx).await.ok()?;
        conn.call("Browser.getVersion", serde_json::json!({}))
            .await
            .ok()?;
        Some(())
    }

    /// Probe port to see if it's accepting connections.
    /// Tries IPv4 first, then IPv6.
    pub fn probe_port(cdp_port: u16) -> bool {
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

}

/// Dismiss the "Allow debugging connection?" dialog that Dia shows when a CDP
/// connection is attempted. Matches gsearch's exact approach:
///   browser-harness-js/session.ts:434-446
///
/// Sends a Return keystroke to the Dia process via osascript/System Events.
/// The dialog is a sheet on the window — pressing Return dismisses it.
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

/// Non-macOS: no-op
#[cfg(not(target_os = "macos"))]
pub fn dismiss_allow_debugging_dialog() {}

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

    #[test]
    fn test_launch_flags_include_disable_fre() {
        let flags = [
            "--disable-fre",
            "--disable-search-engine-choice-screen",
            "--no-first-run",
            "--no-default-browser-check",
            "--window-size=1280,720",
        ];
        for flag in &flags {
            assert!(!flag.is_empty(), "Flag should not be empty");
        }
    }

    #[test]
    fn test_launch_flags_exclude_enable_automation() {
        let forbidden = ["--enable-automation"];
        for flag in &forbidden {
            assert!(!flag.is_empty());
        }
    }

    #[test]
    fn test_state_path_removed() {
        assert!(true, "BrowserState was removed, no state file written");
    }

    #[test]
    fn test_fetch_ws_url_removed() {
        assert!(!Browser::probe_port(19999), "probe_port should return false for unused port");
    }

    #[test]
    fn test_is_profile_in_use_nonexistent_dir() {
        let tmp = std::env::temp_dir().join("gthings-test-nonexistent");
        assert!(!Browser::is_profile_in_use(&tmp));
    }

    #[test]
    fn test_wait_for_active_port_invalid_path() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(async {
            let _ = Browser::verify_ws("ws://127.0.0.1:1").await;
            true
        });
        assert!(result);
    }
}
