use std::sync::OnceLock;

use serde_json::Value;

use super::urls::{http_probe_url, rewrite_ws_host};
use super::{cdp_host, host_header};
use crate::browser::DetectedBrowser;
use crate::error::{CdpError, Result};

/// Shared HTTP client with sensible timeouts.
fn http_client() -> Result<&'static reqwest::Client> {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    Ok(CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(3))
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("failed to build reqwest client")
    }))
}

/// GET the DevTools JSON at `path` with the `Host: localhost` header.
///
/// Returns the parsed JSON body, or `None` when the endpoint is unreachable
/// or does not return JSON.
async fn probe_json(port: u16, path: &str) -> Option<Value> {
    let host = cdp_host();
    let url = http_probe_url(&host, port, path);
    let client = http_client().ok()?;
    let resp = client
        .get(&url)
        .header(reqwest::header::HOST, host_header(port))
        .send()
        .await
        .map_err(|e| tracing::debug!(error = %e, "probe {path} http get failed"))
        .ok()?;
    resp.json()
        .await
        .map_err(|e| tracing::debug!(error = %e, "probe {path} json parse failed"))
        .ok()
}

/// GET `/json/version` with the `Host: localhost` header.
///
/// Returns the detected browser — with the ws URL rewritten to the configured
/// CDP host — or `None` when the endpoint is unreachable or not DevTools.
pub(crate) async fn probe_version(port: u16) -> Option<DetectedBrowser> {
    let host = cdp_host();
    let body = probe_json(port, "/json/version").await?;

    let ws_url = body.get("webSocketDebuggerUrl")?.as_str()?;
    let ws_url = rewrite_ws_host(ws_url, &host);
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
        ws_url,
        browser,
        version,
    })
}

/// GET `/json` or `/json/list` — returns the `webSocketDebuggerUrl` of the
/// first available page target, rewritten to the configured CDP host.
pub(crate) async fn probe_list(port: u16, path: &str) -> Option<String> {
    let host = cdp_host();
    let list = probe_json(port, path).await?;
    let list = list.as_array()?;
    for entry in list {
        if let Some(ws_url) = entry.get("webSocketDebuggerUrl").and_then(|v| v.as_str()) {
            return Some(rewrite_ws_host(ws_url, &host));
        }
    }
    None
}

/// Daemon-side health probe: GET `/json/version` with the `Host: localhost`
/// header. Returns `Ok(())` when the DevTools endpoint responds with JSON.
pub async fn check_alive(port: u16) -> Result<()> {
    if probe_json(port, "/json/version").await.is_some() {
        Ok(())
    } else {
        Err(CdpError::ConnectionFailed {
            detail: "DevTools /json/version probe failed".into(),
        })
    }
}
