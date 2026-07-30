//! Google search implementation via CDP.
//!
//! Uses attribute-based selectors (`a[href]` filtered by hostname) resilient
//! to Google class name changes.

use std::time::{Duration, Instant};

use chrono::Utc;
use gthings_cdp::{CdpError, Session, Tab};
use gthings_common::provenance::{ExtractionMethod, Provenance};
use serde::{Deserialize, Serialize};
/// A single Google search result with provenance metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub position: usize,
    /// How and when this result was obtained.
    #[serde(default, skip)]
    pub provenance: Provenance,
    /// Domain authority score (0.0–1.0) for the result URL's host.
    #[serde(default)]
    pub domain_authority: f32,
}

/// Execute a Google search via CDP.
///
/// Navigates to Google SERP, waits for network idle via lifecycle events,
/// then extracts organic results using in-browser JavaScript with
/// attribute-based selectors.
///
/// If the search returns zero results, the query is **retried once** with a
/// trailing space appended. Google sometimes penalizes bare queries; the
/// trailing space can bypass this.
///
/// # Arguments
///
/// * `session` — The CDP session managing the browser connection.
/// * `tab` — An already-created tab. Will be navigated to the SERP.
/// * `query` — The search query string.
/// * `count` — Maximum number of search results to return.
pub async fn search(
    session: &Session,
    tab: &Tab,
    query: &str,
    count: usize,
) -> Result<Vec<SearchResult>, CdpError> {
    let results = search_once(session, tab, query, count).await?;
    if results.is_empty() {
        // Retry ONCE with trailing space — Google sometimes returns zero
        // results for bare queries that work with a trailing space.
        let spaced = format!("{query} ");
        search_once(session, tab, &spaced, count).await
    } else {
        Ok(results)
    }
}

/// Inner search function (single attempt, no retry).
async fn search_once(
    session: &Session,
    tab: &Tab,
    query: &str,
    count: usize,
) -> Result<Vec<SearchResult>, CdpError> {
    let start = Instant::now();

    let params: String = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("q", query)
        .append_pair("num", &count.to_string())
        .append_pair("hl", "en")
        .finish();
    let url = format!("https://www.google.com/search?{params}");

    tab.navigate(session, &url).await?;

    // Check for Google CAPTCHA/Sorry block
    {
        let page_url = tab.evaluate(session, "window.location.href").await?;
        let current_url = page_url
            .get("result")
            .and_then(|r| r.get("value"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if current_url.contains("/sorry/") || current_url.contains("google.com/sorry") {
            tracing::warn!("Google CAPTCHA/Sorry page detected at: {current_url}");
            return Err(CdpError::CaptchaBlocked {
                detail: format!(
                    "Google served CAPTCHA page instead of search results: {current_url}"
                ),
            });
        }

        // Also check for "Accessibility help" or "Learn more" in page title
        let page_title = tab.evaluate(session, "document.title").await?;
        let title = page_title
            .get("result")
            .and_then(|r| r.get("value"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if title.contains("Accessibility") || title.contains("Learn more") {
            tracing::warn!("Google access-denied page detected: {title}");
            return Err(CdpError::CaptchaBlocked {
                detail: format!(
                    "Google returned access-denied page '{title}' instead of search results"
                ),
            });
        }
    }

    // Scroll down to trigger lazy loading of more organic results.
    // Google SERP only renders ~2-3 results initially; the rest are
    // loaded dynamically when the user scrolls.
    let scroll_iterations = (count / 2).max(1);
    for _ in 0..scroll_iterations {
        tab.evaluate(session, "window.scrollBy(0, 800)").await?;
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // In-browser JS: iterate all links, skip self-hosted, extract snippet
    // via attribute-based selectors (no brittle class-name dependency).
    // Includes timing measurement and resilient selector fallbacks.
    let js = format!(
        r#"
const _st = Date.now();
const count = {};
const links = Array.from(document.querySelectorAll('a[href]'));
const results = [];
for (const a of links) {{
  try {{
    const url = a.href;
    let hostname;
    try {{ hostname = new URL(url).hostname; }} catch(_) {{ continue; }}
    if (hostname === location.hostname) continue;
    const title = a.textContent.trim();
    if (!title || title.length < 2) continue;
    const parent = a.closest('div.g, div[data-hveid], div[data-sokoban-container]');
    const snippetEl = parent?.querySelector('.VwiC3b, [data-sncf], span.aCOpRe, .lEBKkf, span[style*="webkit-line-clamp"]');
    const snippet = (snippetEl?.textContent || '').trim();
    results.push({{ title, url, snippet, position: results.length + 1 }});
    if (results.length >= count) break;
  }} catch(e) {{ continue; }}
}}
console.log('[gthings] search: ' + results.length + ' results in ' + (Date.now() - _st) + 'ms');
JSON.stringify(results);
"#,
        count
    );

    let result = tab.evaluate(session, &js).await?;
    let raw = result["result"]["value"].as_str();
    let json_str = raw.unwrap_or("[]");
    let mut items: Vec<SearchResult> = serde_json::from_str(json_str).map_err(|e| {
        let preview = &json_str[..json_str.len().min(200)];
        tracing::warn!("search: failed to parse results JSON: {e} (preview: {preview:?})");
        CdpError::Json(e)
    })?;

    let duration_ms = start.elapsed().as_millis() as u64;
    let now = Utc::now();

    for item in &mut items {
        let host = gthings_common::extract_host(&item.url).unwrap_or_default();
        item.domain_authority = gthings_extraction::domain_authority(&host);
        item.provenance = Provenance {
            source_url: url.clone(),
            method: ExtractionMethod::Search,
            agent: gthings_common::GTHINGS_AGENT.into(),
            accessed_at: now,
            duration_ms,
            derived_from: None,
        };
    }

    // Post-process: filter junk, dedup by base URL, clean snippets, re-number, round authority
    items.retain(|r| {
        let lower = r.url.to_lowercase();
        if r.url.contains("#:~:text=") {
            return false;
        }
        if lower.starts_with("https://support.google.com/") {
            return false;
        }
        if lower.starts_with("https://accounts.google.com/") {
            return false;
        }
        if lower.starts_with("https://policies.google.com/") {
            return false;
        }
        if lower.contains("doubleclick.net") {
            return false;
        }
        if lower.contains("googlesyndication.com") {
            return false;
        }
        if r.snippet.trim().len() < 5 {
            return false;
        }
        true
    });

    // Dedup by base URL (strip fragment) - prefer main page over section links
    let mut seen_bases: std::collections::HashSet<String> = std::collections::HashSet::new();
    items.retain(|r| {
        let base = r.url.split('#').next().unwrap_or(&r.url).to_string();
        seen_bases.insert(base)
    });

    // Clean snippets: strip trailing "Read more" and "...Read more"
    for item in &mut items {
        let snip = item.snippet.clone();
        item.snippet = if snip.ends_with("...Read more") {
            safe_truncate_end(&snip, "...Read more")
        } else if snip.ends_with("Read more") {
            safe_truncate_end(&snip, "Read more")
        } else {
            snip
        };
    }

    // Clean titles: strip inline URLs and appended domain patterns
    for item in &mut items {
        for prefix in &["https://", "http://"] {
            if let Some(pos) = item.title.find(prefix) {
                item.title = item.title[..pos].trim().to_string();
            }
        }
        // Algorithmic: detect appended domain at title end (e.g. "TitleWikipedia")
        let title_bytes = item.title.as_bytes();
        for i in (1..item.title.len()).rev() {
            let c = title_bytes[i] as char;
            let p = title_bytes[i - 1] as char;
            if c.is_uppercase() && (p.is_lowercase() || p == ')') {
                let suffix = &item.title[i..];
                if suffix.len() >= 3 && suffix.len() <= 25 {
                    item.title = item.title[..i].trim().to_string();
                    break;
                }
            }
        }
        item.title = item.title.trim().to_string();
    }

    // Re-number positions sequentially
    for (i, item) in items.iter_mut().enumerate() {
        item.position = i + 1;
    }

    // Round domain_authority to 2 decimal places
    for item in &mut items {
        item.domain_authority = (item.domain_authority * 100.0).round() / 100.0;
    }

    Ok(items)
}

/// Safely truncate a suffix from the end of a string, respecting UTF-8 character boundaries.
///
/// Returns the remainder of the string (trimmed) if the suffix is present,
/// or the original string unchanged if the suffix is not found.
///
/// Unlike byte-level slicing (`s[..s.len()-n]`), this avoids panicking on
/// multi-byte characters such as non-breaking space (U+00A0), CJK, or emoji.
fn safe_truncate_end(s: &str, suffix: &str) -> String {
    s.strip_suffix(suffix)
        .map(|trimmed| trimmed.trim().to_string())
        .unwrap_or_else(|| s.to_string())
}
