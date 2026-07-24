//! Google search implementation.
//!
//! Provides [`GoogleSearch`] for executing web searches via Google.
//! Each search launches an ephemeral Chrome instance, navigates to the
//! Google SERP, and extracts organic results via CDP JavaScript evaluation.

use std::cmp::Reverse;
use std::time::Instant;

use cdp::{Browser, Tab};

use common::GthingsError;
use common::config::GthingsConfig;
use common::trace::TraceWriter;

use crate::types::{BatchSearchResult, SearchMeta, SearchResult};

/// Google web search client.
///
/// Each [`query`](GoogleSearch::query) call launches an ephemeral Chrome
/// instance, navigates to `google.com/search`, extracts organic results
/// via CDP JavaScript evaluation, then shuts Chrome down on Drop.
pub struct GoogleSearch;

impl GoogleSearch {
    /// Create a new [`GoogleSearch`] instance.
    pub fn new(_config: GthingsConfig) -> Self {
        Self
    }

    /// Execute a single Google search query.
    ///
    /// Launches Chrome, navigates to `google.com/search`, extracts
    /// organic results via CDP JavaScript evaluation.
    ///
    /// # Errors
    ///
    /// Returns [`GthingsError::Cdp`] if Chrome cannot be launched or the
    /// CDP call fails.
    pub async fn query(
        &self,
        q: &str,
        count: usize,
        deny_hosts: Option<&[String]>,
        mut trace: Option<&mut TraceWriter>,
    ) -> Result<Vec<SearchResult>, GthingsError> {
        let start = Instant::now();

        let search_results = self.query_inner(q, count, trace.as_deref_mut()).await?;

        // Empty-result retry: append a trailing space and retry once
        let mut search_results = if search_results.is_empty() && !q.ends_with(' ') {
            let retry_q = format!("{} ", q);
            tracing::debug!(
                query = q,
                "search: empty result, retrying with trailing space"
            );
            match self.query_inner(&retry_q, count, trace).await {
                Ok(retry_results) if !retry_results.is_empty() => retry_results,
                _ => search_results,
            }
        } else {
            search_results
        };

        // Apply deny_hosts filter if provided
        if let Some(hosts) = deny_hosts {
            if !hosts.is_empty() {
                search_results = Self::filter_deny_hosts(search_results, hosts);
            }
        }

        // Rank results
        Self::rank_results(&mut search_results);

        let elapsed = start.elapsed().as_millis() as u64;
        tracing::debug!(
            query = q,
            count = search_results.len(),
            elapsed_ms = elapsed,
            "search: done"
        );

        Ok(search_results)
    }

    /// Inner search that launches Chrome and extracts results.
    async fn query_inner(
        &self,
        q: &str,
        count: usize,
        mut trace: Option<&mut TraceWriter>,
    ) -> Result<Vec<SearchResult>, GthingsError> {
        // Step 1: Browser launch / reuse
        let browser_start = Instant::now();
        let browser = Browser::launch()
            .await
            .map_err(|e| GthingsError::Cdp(format!("Launch: {e}")))?;
        if let Some(ref mut t) = trace {
            t.step(
                "session",
                1,
                "search",
                "browser_reuse",
                None,
                browser_start.elapsed().as_millis() as u64,
                None,
                None,
                None,
            );
        }

        let _conn_start = Instant::now();
        let mut conn = browser
            .connect()
            .await
            .map_err(|e| GthingsError::Cdp(format!("Connect: {e}")))?;

        // Step 2: Tab create
        let tab_start = Instant::now();
        let tab = Tab::create(&mut conn, browser.ws_url(), "about:blank")
            .await
            .map_err(|e| GthingsError::Cdp(format!("CreateTab: {e}")))?;
        if let Some(ref mut t) = trace {
            t.step(
                "session",
                2,
                "search",
                "tab_create",
                None,
                tab_start.elapsed().as_millis() as u64,
                None,
                None,
                None,
            );
        }

        let encoded = urlencoding::encode(q);
        let url = format!("https://www.google.com/search?q={}&num={}", encoded, count);

        // Step 3: Navigate
        let nav_start = Instant::now();
        tab.navigate(&mut conn, &url)
            .await
            .map_err(|e| GthingsError::Cdp(format!("Navigate: {e}")))?;
        if let Some(ref mut t) = trace {
            t.step(
                "session",
                3,
                "search",
                "navigate",
                Some(&url),
                nav_start.elapsed().as_millis() as u64,
                Some(serde_json::json!({"url": url})),
                None,
                None,
            );
        }

        // Allow Google to render
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        // Extract organic results via JS
        let js = format!(
            r#"
(() => {{
    const results = [];
    const selectors = [
        'div.g',
        'div[data-hveid]',
        'div.yuRUbf',
    ];
    const seen = new Set();
    for (const sel of selectors) {{
        const items = document.querySelectorAll(sel);
        for (const item of items) {{
            const titleEl = item.querySelector('h3');
            const linkEl = item.querySelector('a[href^="http"]');
            const snippetEl = item.querySelector('.VwiC3b, .st, [data-sncf], .lEBKkf span');
            if (titleEl && linkEl) {{
                const url = linkEl.href || '';
                if (seen.has(url)) continue;
                seen.add(url);
                results.push({{
                    title: (titleEl.innerText || titleEl.textContent || '').trim(),
                    url: url,
                    snippet: (snippetEl?.innerText || snippetEl?.textContent || '').trim(),
                }});
            }}
        }}
    }}
    return JSON.stringify(results.slice(0, {}));
}})()
"#,
            count
        );

        // Step 4: Extract
        let ext_start = Instant::now();
        let result = tab
            .evaluate(&mut conn, &js)
            .await
            .map_err(|e| GthingsError::Cdp(format!("Eval: {e}")))?;

        let json_str = result["result"]["value"].as_str().unwrap_or("[]");

        let items: Vec<serde_json::Value> = serde_json::from_str(json_str).unwrap_or_default();

        let search_results: Vec<SearchResult> = items
            .into_iter()
            .map(|item| {
                let mut result: SearchResult = serde_json::from_value(item).unwrap_or_default();
                result.query = Some(q.to_string());
                result
            })
            .collect();

        if let Some(ref mut t) = trace {
            t.step("session", 4, "search", "extract", Some(&url),
                ext_start.elapsed().as_millis() as u64,
                None,
                Some(serde_json::json!({"result_count": search_results.len(), "titles": search_results.iter().map(|r| &r.title).collect::<Vec<_>>()})),
                None);
        }

        // Step 5: Close tab
        let close_start = Instant::now();
        let _ = tab.close(&mut conn).await;
        if let Some(ref mut t) = trace {
            t.step(
                "session",
                5,
                "search",
                "tab_close",
                None,
                close_start.elapsed().as_millis() as u64,
                None,
                None,
                None,
            );
        }

        Ok(search_results)
    }

    /// Filter out results from denied hostnames.
    pub fn filter_deny_hosts(
        results: Vec<SearchResult>,
        deny_hosts: &[String],
    ) -> Vec<SearchResult> {
        results
            .into_iter()
            .filter(|r| {
                if let Ok(url) = url::Url::parse(&r.url) {
                    let host = url.host_str().unwrap_or("");
                    !deny_hosts
                        .iter()
                        .any(|d| host == d || host.ends_with(&format!(".{}", d)))
                } else {
                    true
                }
            })
            .collect()
    }

    /// Rank results by quality score.
    /// Prefers: organic blocks > longer snippets > https > shorter URLs.
    pub fn rank_results(results: &mut [SearchResult]) {
        results.sort_by(|a, b| {
            // Prefer longer snippets (more context)
            let snippet_cmp = b.snippet.len().cmp(&a.snippet.len());
            // Tiebreak: prefer https
            let a_https = a.url.starts_with("https://");
            let b_https = b.url.starts_with("https://");
            let https_cmp = b_https.cmp(&a_https);
            snippet_cmp.then(https_cmp)
        });
    }

    /// Batch search multiple queries.
    ///
    /// Each query is sent individually. Results are
    /// deduplicated by URL and ranked by snippet length (descending).
    ///
    /// # Errors
    ///
    /// Returns [`GthingsError`] if any request fails.
    pub async fn batch(
        &self,
        queries: &[String],
        count: usize,
        deny_hosts: Option<&[String]>,
        mut trace: Option<&mut TraceWriter>,
    ) -> Result<BatchSearchResult, GthingsError> {
        if queries.is_empty() {
            return Ok(BatchSearchResult {
                results: Vec::new(),
                meta: SearchMeta {
                    total: 0,
                    query: String::new(),
                    duration_ms: 0,
                },
            });
        }

        let start = Instant::now();
        let mut all_results = Vec::new();

        for q in queries {
            let mut results = self
                .query(q, count, deny_hosts, trace.as_deref_mut())
                .await?;
            all_results.append(&mut results);
        }

        // Dedup by URL
        let mut seen = std::collections::HashSet::new();
        all_results.retain(|r| seen.insert(r.url.clone()));

        all_results.sort_by_key(|k| Reverse(k.snippet.len()));

        let elapsed = start.elapsed().as_millis() as u64;
        let total = all_results.len();
        let query_label = if queries.len() == 1 {
            queries[0].clone()
        } else {
            format!("{} queries", queries.len())
        };

        tracing::debug!(
            n_results = total,
            elapsed_ms = elapsed,
            "batch-search: done"
        );

        Ok(BatchSearchResult {
            results: all_results,
            meta: SearchMeta {
                total,
                query: query_label,
                duration_ms: elapsed,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SearchResult;

    #[test]
    fn test_filter_deny_hosts_removes_matching() {
        let results = vec![
            SearchResult {
                title: "A".into(),
                url: "https://example.com/page".into(),
                snippet: "...".into(),
                query: None,
            },
            SearchResult {
                title: "B".into(),
                url: "https://evil.com/page".into(),
                snippet: "...".into(),
                query: None,
            },
            SearchResult {
                title: "C".into(),
                url: "https://example.org/page".into(),
                snippet: "...".into(),
                query: None,
            },
        ];
        let deny = vec!["evil.com".to_string()];
        let filtered = GoogleSearch::filter_deny_hosts(results, &deny);
        assert_eq!(filtered.len(), 2);
        assert!(filtered.iter().all(|r| !r.url.contains("evil.com")));
    }

    #[test]
    fn test_filter_deny_hosts_empty_deny() {
        let results = vec![SearchResult {
            title: "A".into(),
            url: "https://example.com/page".into(),
            snippet: "...".into(),
            query: None,
        }];
        let deny: Vec<String> = vec![];
        let filtered = GoogleSearch::filter_deny_hosts(results, &deny);
        assert_eq!(filtered.len(), 1);
    }

    #[test]
    fn test_filter_deny_hosts_empty_results() {
        let results: Vec<SearchResult> = vec![];
        let deny = vec!["evil.com".to_string()];
        let filtered = GoogleSearch::filter_deny_hosts(results, &deny);
        assert!(filtered.is_empty());
    }

    #[test]
    fn test_rank_results_does_not_panic() {
        let mut results = vec![
            SearchResult {
                title: "B".into(),
                url: "https://b.com".into(),
                snippet: "...".into(),
                query: None,
            },
            SearchResult {
                title: "A".into(),
                url: "https://a.com".into(),
                snippet: "...".into(),
                query: None,
            },
        ];
        GoogleSearch::rank_results(&mut results);
        assert_eq!(results.len(), 2);
    }
}
