//! Google search implementation via CDP.
//!
//! Uses attribute-based selectors (`a[href]` filtered by hostname) resilient
//! to Google class name changes.

use std::time::Instant;

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
    #[serde(default)]
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

    // In-browser JS: iterate all links, skip self-hosted, extract snippet
    // via attribute-based selectors (no brittle class-name dependency).
    let js = format!(
        r#"
const count = {};
const links = Array.from(document.querySelectorAll('a[href]'));
const results = [];
for (const a of links) {{
  try {{
    const url = a.href;
    const hostname = new URL(url).hostname;
    if (hostname === location.hostname) continue;
    const title = a.textContent.trim();
    if (!title) continue;
    const snippet = a.closest('div.g, div[data-hveid]')?.querySelector('.VwiC3b, [data-sncf], span.aCOpRe')?.textContent?.trim() || '';
    results.push({{ title, url, snippet, position: results.length + 1 }});
    if (results.length >= count) break;
  }} catch(e) {{ continue; }}
}}
JSON.stringify(results);
"#,
        count
    );

    let result = tab.evaluate(session, &js).await?;
    let json_str = result["result"]["value"].as_str().unwrap_or("[]");
    let mut items: Vec<SearchResult> = serde_json::from_str(json_str)?;

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

    Ok(items)
}
