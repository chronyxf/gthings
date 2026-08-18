use std::path::PathBuf;

use crate::browser::DetectedBrowser;

// ---------------------------------------------------------------------------
// Internal probe helpers
// ---------------------------------------------------------------------------

/// Scan well-known browser profile directories for a `DevToolsActivePort`
/// file whose port matches the requested port, then verify via TCP connect.
pub(crate) async fn probe_devtools_active_port(port: u16) -> Option<DetectedBrowser> {
    let profile_dirs = get_profile_dirs();
    if profile_dirs.is_empty() {
        return None;
    }

    // Synchronous file reads are fine here — negligible for a handful of files.
    let result: Option<DetectedBrowser> = tokio::task::spawn_blocking(move || {
        let host = crate::discovery::cdp_host();
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
            let file_port: u16 = match lines[0].trim().parse() {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(error = %e, "failed to parse port from DevToolsActivePort");
                    continue;
                }
            };
            if file_port != port {
                continue;
            }
            let ws_path = lines[1].trim();
            let ws_url = crate::discovery::ws_probe_url(&host, port, ws_path);

            // Verify port is accepting TCP connections
            let addr = match crate::discovery::cdp_socket_addr(&host, port) {
                Some(a) => a,
                None => continue,
            };
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
    // Map from path suffix keywords to browser display names.
    // Order matters: check more specific patterns first.
    const BROWSER_MAP: &[(&str, &str)] = &[
        ("Chrome Canary", "Google Chrome Canary"),
        ("Chrome", "Google Chrome"),
        ("Chromium", "Chromium"),
        ("Edge Canary", "Microsoft Edge Canary"),
        ("Edge", "Microsoft Edge"),
        ("Brave", "Brave"),
        ("Arc", "Arc"),
        ("Vivaldi", "Vivaldi"),
        ("Opera", "Opera"),
        ("Dia", "Dia"),
    ];
    for (keyword, name) in BROWSER_MAP {
        if s.contains(keyword) {
            return name.to_string();
        }
    }
    "unknown".into()
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
