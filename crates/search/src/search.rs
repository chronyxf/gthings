//! Google search implementation via CDP.
//!
//! Uses attribute-based selectors (`a[href]` filtered by hostname) resilient
//! to Google class name changes.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use chrono::Utc;
use gthings_cdp::{CdpError, Session, Tab};
use gthings_common::provenance::{ExtractionMethod, Provenance};
use gthings_common::safe_truncate_end;
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
        let spaced = if !query.ends_with(' ') {
            format!("{query} ")
        } else {
            query.to_string()
        };
        search_once(session, tab, &spaced, count).await
    } else {
        Ok(results)
    }
}

/// Inner search function (single attempt, no retry).
#[allow(clippy::incompatible_msrv)]
async fn search_once(
    session: &Session,
    tab: &Tab,
    query: &str,
    count: usize,
) -> Result<Vec<SearchResult>, CdpError> {
    let start = Instant::now();

    let params: String = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("q", query)
        .append_pair("num", &(count * 2).max(10).to_string())
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
    // 200ms sleep per iteration is empirically sufficient for
    // lazy-loading Google SERP results on modern connections;
    // originally 500ms — reduced as a simple latency optimization.
    let scroll_iterations = count.max(3);
    for _ in 0..scroll_iterations {
        tab.evaluate(session, "window.scrollBy(0, 800)").await?;
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // In-browser JS: iterate all links, skip self-hosted, extract snippet
    // via attribute-based selectors (no brittle class-name dependency).
    // Includes timing measurement and resilient selector fallbacks.
    let js =
        include_str!("../templates/search_extract.js").replace("__COUNT__", &count.to_string());

    let result = tab.evaluate(session, &js).await?;
    let raw = result["result"]["value"].as_str();
    let json_str = raw.unwrap_or("[]");
    let mut items: Vec<SearchResult> = serde_json::from_str(json_str).map_err(|e| {
        let preview = &json_str[..json_str.floor_char_boundary(json_str.len().min(200))];
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

    post_process_search_results(&mut items);

    Ok(items)
}

/// Post-process search results: filter junk URLs, deduplicate by base URL,
/// clean snippets/titles, re-number positions, and round authority scores.
#[allow(clippy::incompatible_msrv)]
fn post_process_search_results(items: &mut Vec<SearchResult>) {
    // Filter junk, fragments, and short snippets, then dedup by base URL
    let mut seen_bases: HashSet<String> = HashSet::new();
    items.retain(|r| {
        if r.url.contains("#:~:text=") {
            return false;
        }
        if crate::harvest::is_junk_url(&r.url) {
            return false;
        }
        if r.snippet.trim().is_empty() {
            return false;
        }
        let base = r.url.split('#').next().unwrap_or(&r.url).to_string();
        seen_bases.insert(base)
    });

    // Combine snippet cleaning, title cleaning, re-numbering, and rounding
    for (i, item) in items.iter_mut().enumerate() {
        // Clean snippet: strip trailing "Read more" and "...Read more"
        let snip = &item.snippet;
        if snip.ends_with("...Read more") {
            item.snippet = safe_truncate_end(snip, "...Read more");
        } else if snip.ends_with("Read more") {
            item.snippet = safe_truncate_end(snip, "Read more");
        }

        // Clean title: strip inline URLs and appended domain patterns
        for prefix in &["https://", "http://"] {
            if let Some(pos) = item.title.find(prefix) {
                let safe_pos = item.title.floor_char_boundary(pos);
                item.title = item.title[..safe_pos].trim().to_string();
            }
        }
        // Detect appended domain at title end (e.g. "TitleWikipedia")
        // Scan forward to find the last lowercase→uppercase transition where
        // the uppercase suffix is 3-25 chars long.
        let mut truncate_at: Option<usize> = None;
        for (i, c) in item.title.char_indices() {
            if i == 0 || !c.is_uppercase() {
                continue;
            }
            let prev = item.title[..i].chars().last().unwrap();
            if !(prev.is_lowercase() || prev == ')') {
                continue;
            }
            let suffix = &item.title[i..];
            if (3..=25).contains(&suffix.len()) {
                truncate_at = Some(i);
            }
        }
        if let Some(pos) = truncate_at {
            item.title = item.title[..pos].trim().to_string();
        }
        item.title = item.title.trim().to_string();

        // Re-number position sequentially
        item.position = i + 1;

        // Round domain_authority to 2 decimal places
        item.domain_authority = (item.domain_authority * 100.0).round() / 100.0;
    }
}
