use std::path::{Path, PathBuf};
use std::time::Duration;

use common::GthingsError;

/// Chromium-based browser detection and management.
pub struct ChromeInstance;

impl ChromeInstance {
    /// Find any Chromium-based browser executable on the system.
    /// Returns the first one found, in priority order.
    pub fn find_executable() -> Option<PathBuf> {
        // 1. Check CHROME_PATH env var (highest priority)
        if let Ok(path) = std::env::var("CHROME_PATH") {
            let p = PathBuf::from(&path);
            if p.exists() {
                return Some(p);
            }
        }

        // 2. macOS: common Application paths
        #[cfg(target_os = "macos")]
        {
            let candidates = [
                "Google Chrome.app/Contents/MacOS/Google Chrome",
                "Chromium.app/Contents/MacOS/Chromium",
                "Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
                "Brave Browser.app/Contents/MacOS/Brave Browser",
                "Opera.app/Contents/MacOS/Opera",
                "Vivaldi.app/Contents/MacOS/Vivaldi",
                "Arc.app/Contents/MacOS/Arc",
                "Dia.app/Contents/MacOS/Dia",
            ];
            let home = whoami();
            let user_apps = format!("/Users/{home}/Applications");
            let app_dirs = [Path::new("/Applications"), Path::new(&user_apps)];
            for app_dir in &app_dirs {
                for candidate in &candidates {
                    let p = app_dir.join(candidate);
                    if p.exists() {
                        return Some(p);
                    }
                }
            }

            // 3. mdfind (Spotlight) — find any Chromium-based browser
            if let Some(path) = Self::find_by_spotlight() {
                return Some(path);
            }
        }

        // 4. Linux: PATH lookup
        #[cfg(target_os = "linux")]
        {
            let path_candidates = [
                "google-chrome",
                "google-chrome-stable",
                "chromium",
                "chromium-browser",
                "microsoft-edge",
                "brave-browser",
                "opera",
                "vivaldi",
            ];
            for name in &path_candidates {
                if let Ok(output) = Command::new("which").arg(name).output() {
                    if output.status.success() {
                        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                        if !path.is_empty() {
                            return Some(PathBuf::from(path));
                        }
                    }
                }
            }
        }

        None
    }

    /// macOS: Use mdfind to find Chromium-based browsers via Spotlight.
    #[cfg(target_os = "macos")]
    fn find_by_spotlight() -> Option<PathBuf> {
        use std::process::Command;

        // Check common bundle identifiers
        let bundle_ids = [
            "com.google.Chrome",
            "com.microsoft.edgemac",
            "com.brave.Browser",
            "com.operasoftware.Opera",
            "com.vivaldi.Vivaldi",
            "company.Arc",
            "com.dia.browser",
            "org.chromium.Chromium",
        ];

        for bundle_id in &bundle_ids {
            let query = format!(
                "kMDItemContentType == 'com.apple.application-bundle' && \
                 kMDItemCFBundleIdentifier == '{bundle_id}'"
            );
            if let Ok(output) = Command::new("mdfind").arg(&query).output() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    let app_path = line.trim();
                    if app_path.is_empty() {
                        continue;
                    }
                    let stem = Path::new(app_path).file_stem()?.to_str()?.to_string();
                    let binary = Path::new(app_path).join("Contents/MacOS").join(&stem);
                    if binary.exists() {
                        if Self::is_chromium_binary(&binary) {
                            return Some(binary);
                        }
                    }
                }
            }
        }

        // Broader: mdfind for any Application, then filter by Chromium capability
        if let Ok(output) = Command::new("mdfind")
            .arg("kMDItemKind == 'Application'")
            .output()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                let app_path = line.trim();
                if app_path.is_empty() {
                    continue;
                }
                let stem = Path::new(app_path).file_stem()?.to_str()?.to_string();
                let binary = Path::new(app_path).join("Contents/MacOS").join(&stem);
                if binary.exists() && Self::is_chromium_binary(&binary) {
                    return Some(binary);
                }
            }
        }

        None
    }

    /// Check if a binary is Chromium-based by looking for CDP support.
    fn is_chromium_binary(path: &Path) -> bool {
        if let Ok(output) = std::process::Command::new(path).arg("--help").output() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let help = format!("{stdout}{stderr}");
            if help.contains("remote-debugging-port") {
                return true;
            }
        }
        false
    }

    /// macOS: Find the user's default browser via LaunchServices plist.
    #[cfg(target_os = "macos")]
    fn find_default_browser() -> Option<PathBuf> {
        use std::process::Command;

        let home = whoami();
        let plist_path = PathBuf::from(format!(
            "/Users/{home}/Library/Preferences/com.apple.LaunchServices/\
             com.apple.launchservices.secure.plist"
        ));

        if !plist_path.exists() {
            return None;
        }

        // Use plutil to convert plist to JSON, then parse
        if let Ok(output) = Command::new("plutil")
            .args(["-convert", "json", "-o", "-"])
            .arg(&plist_path)
            .output()
        {
            if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&output.stdout) {
                if let Some(handlers) = json.get("LSHandlers").and_then(|h| h.as_array()) {
                    for handler in handlers {
                        let scheme = handler
                            .get("LSHandlerURLScheme")
                            .and_then(|s| s.as_str())
                            .unwrap_or("");
                        if scheme == "https" {
                            if let Some(bundle_id) =
                                handler.get("LSHandlerRoleAll").and_then(|b| b.as_str())
                            {
                                let query = format!("kMDItemCFBundleIdentifier == '{bundle_id}'");
                                if let Ok(output) = Command::new("mdfind").arg(&query).output() {
                                    let stdout = String::from_utf8_lossy(&output.stdout);
                                    if let Some(app_path) = stdout.lines().next() {
                                        let app_path = app_path.trim();
                                        if !app_path.is_empty() {
                                            let stem = Path::new(app_path).file_stem()?.to_str()?;
                                            let binary = Path::new(app_path)
                                                .join("Contents/MacOS")
                                                .join(stem);
                                            if binary.exists() {
                                                return Some(binary);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        None
    }

    /// Discover Chrome WebSocket URL from HTTP endpoint.
    /// Tries /json/version first, falls back to /json and /json/list.
    /// Then reads DevToolsActivePort files (Dia Browser doesn't expose HTTP
    /// endpoints but writes its WS path to DevToolsActivePort).
    pub async fn discover(port: u16) -> Result<String, GthingsError> {
        // Try /json/version (standard, works for Chrome, Edge, Brave)
        let url = format!("http://127.0.0.1:{port}/json/version");
        match reqwest::get(&url).await {
            Ok(resp) if resp.status().is_success() => {
                if let Ok(json) = resp.json::<serde_json::Value>().await {
                    if let Some(ws_url) = json["webSocketDebuggerUrl"].as_str() {
                        return Ok(ws_url.to_string());
                    }
                }
            }
            _ => {}
        }

        // Try /json (Dia Browser returns 404 on /json/version but works on /json)
        let url = format!("http://127.0.0.1:{port}/json");
        match reqwest::get(&url).await {
            Ok(resp) if resp.status().is_success() => {
                let targets = resp
                    .json::<Vec<serde_json::Value>>()
                    .await
                    .unwrap_or_default();
                for target in &targets {
                    if target["type"].as_str() == Some("browser") {
                        if let Some(ws_url) = target["webSocketDebuggerUrl"].as_str() {
                            return Ok(ws_url.to_string());
                        }
                    }
                }
                // Fallback: use first page's webSocketDebuggerUrl as browser endpoint
                if let Some(first) = targets.first() {
                    if let Some(ws_url) = first["webSocketDebuggerUrl"].as_str() {
                        return Ok(ws_url.to_string());
                    }
                }
            }
            _ => {}
        }

        // Try /json/list as last resort
        let url = format!("http://127.0.0.1:{port}/json/list");
        match reqwest::get(&url).await {
            Ok(resp) if resp.status().is_success() => {
                let targets = resp
                    .json::<Vec<serde_json::Value>>()
                    .await
                    .unwrap_or_default();
                if let Some(first) = targets.first() {
                    if let Some(ws_url) = first["webSocketDebuggerUrl"].as_str() {
                        return Ok(ws_url.to_string());
                    }
                }
            }
            _ => {}
        }

        // Fallback: read DevToolsActivePort files (Dia Browser).
        // Dia doesn't expose /json/* HTTP endpoints but writes the WS path
        // to its profile's DevToolsActivePort file.
        let home = std::env::var("HOME").unwrap_or_default();
        let active_port_paths = [
            format!("{home}/Library/Application Support/Dia/User Data/DevToolsActivePort"),
            format!("{home}/Library/Application Support/Dia/DevToolsActivePort"),
            format!("{home}/Library/Application Support/Google/Chrome/DevToolsActivePort"),
            format!("{home}/Library/Application Support/Chromium/DevToolsActivePort"),
            format!("{home}/Library/Application Support/Microsoft Edge/DevToolsActivePort"),
            format!(
                "{home}/Library/Application Support/BraveSoftware/Brave-Browser/DevToolsActivePort"
            ),
        ];

        for path in &active_port_paths {
            if let Ok(content) = tokio::fs::read_to_string(path).await {
                let lines: Vec<&str> = content.trim().lines().collect();
                if lines.len() >= 2 {
                    if let Ok(file_port) = lines[0].trim().parse::<u16>() {
                        let ws_path = lines[1].trim();
                        if file_port == port && ws_path.starts_with("/devtools/") {
                            return Ok(format!("ws://127.0.0.1:{port}{ws_path}"));
                        }
                    }
                }
            }
        }

        Err(GthingsError::BrowserNotFound(port))
    }

    /// Scan DevToolsActivePort files to discover Chrome instances.
    /// Chromium-based browsers write the active port to <profile>/DevToolsActivePort.
    pub async fn find_active_port() -> Option<u16> {
        let home = std::env::var("HOME").ok()?;
        let search_dirs = [
            format!("{home}/Library/Application Support/Google/Chrome"),
            format!("{home}/Library/Application Support/Chromium"),
            format!("{home}/Library/Application Support/Microsoft Edge"),
            format!("{home}/Library/Application Support/BraveSoftware/Brave-Browser"),
            format!("{home}/Library/Application Support/Dia"),
            format!("{home}/Library/Application Support/Dia/User Data"),
            format!("{home}/Library/Application Support/com.operasoftware.Opera"),
            format!("{home}/Library/Application Support/Vivaldi"),
        ];

        for dir in &search_dirs {
            let active_port = Path::new(dir).join("DevToolsActivePort");
            if active_port.exists() {
                if let Ok(content) = std::fs::read_to_string(&active_port) {
                    if let Some(port_str) = content.lines().next() {
                        if let Ok(port) = port_str.trim().parse::<u16>() {
                            let verify_url = format!("http://127.0.0.1:{port}/json/version");
                            if reqwest::get(&verify_url).await.is_ok() {
                                return Some(port);
                            }
                        }
                    }
                }
            }
        }
        None
    }

    /// Launch a new Chrome instance with remote debugging enabled.
    pub async fn launch(
        port: u16,
        chrome_path: Option<&Path>,
        profile_dir: Option<&Path>,
    ) -> Result<(tokio::process::Child, String), GthingsError> {
        let executable = match chrome_path {
            Some(p) if p.exists() => p.to_path_buf(),
            _ => Self::find_executable().ok_or_else(|| GthingsError::BrowserNotFound(port))?,
        };

        let user_data_dir = match profile_dir {
            Some(d) => d.to_path_buf(),
            None => std::env::temp_dir().join(format!("gthings-profile-{port}")),
        };

        std::fs::create_dir_all(&user_data_dir).ok();

        let mut cmd = tokio::process::Command::new(&executable);
        cmd.args([
            &format!("--remote-debugging-port={port}"),
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-fre",
            &format!("--user-data-dir={}", user_data_dir.display()),
            "--window-size=1920,1080",
            "--disable-sync",
        ]);
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::piped());

        let child = cmd
            .spawn()
            .map_err(|e| GthingsError::Other(format!("Failed to launch browser: {e}")))?;

        // Poll HTTP endpoint instead of parsing stderr (more reliable)
        let mut ws_url = None;
        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            match Self::discover(port).await {
                Ok(url) => {
                    ws_url = Some(url);
                    break;
                }
                Err(_) => continue,
            }
        }

        let ws_url = ws_url.ok_or_else(|| {
            GthingsError::Other("Browser started but no CDP endpoint detected after 10s".into())
        })?;

        Ok((child, ws_url))
    }
}

#[cfg(target_os = "macos")]
fn whoami() -> String {
    std::env::var("USER").unwrap_or_else(|_| "unknown".to_string())
}
