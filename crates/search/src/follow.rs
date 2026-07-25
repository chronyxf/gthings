//! Page following and content extraction via CDP.
//!
//! Navigates to a URL, waits for network idle, then extracts title and body
//! text via in-browser JavaScript evaluation.

use gthings_cdp::{CdpError, Session, Tab};
use serde::{Deserialize, Serialize};

/// Result of following a URL and extracting its content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FollowResult {
    /// The URL that was followed.
    #[serde(default)]
    pub url: String,
    /// Document title from `<title>`.
    #[serde(default)]
    pub title: String,
    /// Extracted page text (up to `max_chars`).
    #[serde(default)]
    pub content: String,
    /// Whether the page text was truncated (exceeded `max_chars`).
    #[serde(default)]
    pub truncated: bool,
    /// Non-empty if the in-browser JS evaluation threw an error.
    #[serde(default)]
    pub error: String,
}

/// Follow a URL and extract page content via CDP.
///
/// Navigates to the URL, waits for network idle via lifecycle events,
/// then extracts the page title and body text (up to `max_chars`) via
/// in-browser JS evaluation.
///
/// # Arguments
///
/// * `session` — The CDP session managing the browser connection.
/// * `tab` — An already-created tab. Will be navigated to `url`.
/// * `url` — The URL to fetch.
/// * `max_chars` — Maximum characters of body text to return.
pub async fn follow(
    session: &Session,
    tab: &Tab,
    url: &str,
    max_chars: usize,
) -> Result<FollowResult, CdpError> {
    tab.navigate(session, url).await?;

    let js = format!(
        r#"try {{ let title = document.title || ''; let text = document.body ? document.body.innerText : ''; JSON.stringify({{title, content: text.substring(0, {}), truncated: text.length > {}}}) }} catch(e) {{ JSON.stringify({{title: document.title || '', content: '', truncated: false, error: e.message}}) }}"#,
        max_chars, max_chars
    );

    let result = tab.evaluate(session, &js).await?;
    let json_str = result["result"]["value"].as_str().unwrap_or("{}");
    let mut follow_result: FollowResult = serde_json::from_str(json_str)?;
    follow_result.url = url.to_string();
    Ok(follow_result)
}
