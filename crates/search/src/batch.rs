//! Batch operations — search, follow, and the two-phase harvest pipeline.
//!
//! Provides [`BatchProcessor`] which orchestrates multi-query searches,
//! multi-URL page following, and a two-phase harvest pipeline.
//!
//! Each operation launches a single Chrome instance and reuses it across
//! all queries/URLs in the batch.

use std::cmp::Reverse;
use std::time::Instant;

use cdp::{Browser, Tab};

use common::GthingsError;
use common::config::GthingsConfig;
use common::trace::TraceWriter;
use extraction::quality::ContentQuality;

use crate::types::{
    BatchSearchResult, FollowOpts, FollowResult, HarvestMeta, HarvestResult, SearchMeta,
    SearchResult,
};

/// Batch processor for multi-step search pipelines.
///
/// Each operation launches a single Chrome instance and reuses it across
/// all queries/URLs in the batch.
///
/// - **`search`** — multi-query search with dedup and ranking.
/// - **`follow`** — multi-URL page extraction with caching.
/// - **`harvest`** — two-phase pipeline (search then follow).
pub struct BatchProcessor {
    config: GthingsConfig,
}

impl BatchProcessor {
    /// Create a new [`BatchProcessor`].
    pub fn new(config: GthingsConfig) -> Self {
        Self { config }
    }

    /// Batch search: run multiple queries using one shared Chrome instance,
    /// deduplicate by URL, rank by snippet length descending.
    ///
    /// # Errors
    ///
    /// Returns [`GthingsError::Cdp`] if Chrome cannot be launched.
    pub async fn search(
        &self,
        queries: &[String],
        count: usize,
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

        let browser = Browser::launch()
            .await
            .map_err(|e| GthingsError::Cdp(format!("Launch: {e}")))?;
        let mut conn = browser
            .connect()
            .await
            .map_err(|e| GthingsError::Cdp(format!("Connect: {e}")))?;

        if let Some(ref mut t) = trace {
            t.step(
                "session",
                0,
                "batch_search",
                "launch",
                None,
                start.elapsed().as_millis() as u64,
                None,
                None,
                None,
            );
        }

        let mut all_results: Vec<SearchResult> = Vec::new();

        for q in queries {
            let tab = Tab::create(&mut conn, browser.ws_url(), "about:blank")
                .await
                .map_err(|e| GthingsError::Cdp(format!("CreateTab: {e}")))?;

            let encoded = urlencoding::encode(q);
            let url = format!("https://www.google.com/search?q={}&num={}", encoded, count);

            tab.navigate(&mut conn, &url)
                .await
                .map_err(|e| GthingsError::Cdp(format!("Navigate: {e}")))?;

            tokio::time::sleep(std::time::Duration::from_secs(2)).await;

            // Extract via JS (same script as search.rs)
            let js = format!(
                r#"
(() => {{
    const results = [];
    const selectors = [
        'div.g', 'div[data-hveid]', 'div.yuRUbf',
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

            let result = tab
                .evaluate(&mut conn, &js)
                .await
                .map_err(|e| GthingsError::Cdp(format!("Eval: {e}")))?;

            let json_str = result["result"]["value"].as_str().unwrap_or("[]");
            let items: Vec<serde_json::Value> = serde_json::from_str(json_str).unwrap_or_default();

            for item in items {
                all_results.push(SearchResult {
                    title: item["title"].as_str().unwrap_or("").to_string(),
                    url: item["url"].as_str().unwrap_or("").to_string(),
                    snippet: item["snippet"].as_str().unwrap_or("").to_string(),
                    query: Some(q.to_string()),
                });
            }

            // Close the tab
            let _ = tab.close(&mut conn).await;
        }

        // Dedup by URL
        let mut seen = std::collections::HashSet::new();
        all_results.retain(|r| seen.insert(r.url.clone()));

        // Sort by snippet length descending
        all_results.sort_by_key(|k| Reverse(k.snippet.len()));

        let elapsed = start.elapsed().as_millis() as u64;
        let total = all_results.len();

        Ok(BatchSearchResult {
            results: all_results,
            meta: SearchMeta {
                total,
                query: format!("{} queries", queries.len()),
                duration_ms: elapsed,
            },
        })
    }

    /// Batch follow: follow multiple URLs using one shared Chrome instance.
    ///
    /// # Errors
    ///
    /// Returns [`GthingsError::Cdp`] if Chrome cannot be launched.
    pub async fn follow(
        &self,
        urls: &[String],
        opts: FollowOpts,
        mut trace: Option<&mut TraceWriter>,
    ) -> Result<Vec<FollowResult>, GthingsError> {
        if urls.is_empty() {
            return Ok(Vec::new());
        }

        let browser = Browser::launch()
            .await
            .map_err(|e| GthingsError::Cdp(format!("Launch: {e}")))?;
        let mut conn = browser
            .connect()
            .await
            .map_err(|e| GthingsError::Cdp(format!("Connect: {e}")))?;

        if let Some(ref mut t) = trace {
            t.step(
                "session",
                0,
                "batch_follow",
                "launch",
                None,
                0,
                None,
                None,
                None,
            );
        }

        let mut pages: Vec<FollowResult> = Vec::with_capacity(urls.len());

        for url in urls {
            let tab = Tab::create(&mut conn, browser.ws_url(), "about:blank")
                .await
                .map_err(|e| GthingsError::Cdp(format!("CreateTab: {e}")))?;

            tab.navigate(&mut conn, url)
                .await
                .map_err(|e| GthingsError::Cdp(format!("Navigate: {e}")))?;

            tokio::time::sleep(std::time::Duration::from_secs(3)).await;

            let html = tab
                .extract_html(&mut conn)
                .await
                .map_err(|e| GthingsError::Cdp(format!("Html: {e}")))?;

            let selector = if opts.selector.is_empty() {
                "body"
            } else {
                &opts.selector
            };

            let extracted = match extraction::html::HtmlExtractor::extract(&html, selector) {
                Ok(ex) => ex,
                Err(e) => {
                    let _ = tab.close(&mut conn).await;
                    pages.push(FollowResult {
                        success: false,
                        url: url.clone(),
                        content: None,
                        total_length: 0,
                        offset: opts.offset,
                        truncated: false,
                        sections: Vec::new(),
                        error: Some(format!("Extract: {e}")),
                        quality: None,
                    });
                    continue;
                }
            };

            let (content, truncated) =
                if opts.offset > 0 || opts.max_length < extracted.content.len() {
                    let start = opts.offset.min(extracted.content.len());
                    let end = (start + opts.max_length).min(extracted.content.len());
                    (extracted.content[start..end].to_string(), true)
                } else {
                    (extracted.content, false)
                };

            let result = FollowResult {
                success: !content.is_empty(),
                url: url.clone(),
                content: Some(content),
                total_length: extracted.total_length,
                offset: opts.offset,
                truncated,
                sections: extracted.sections,
                error: None,
                quality: Some(ContentQuality::validate("")), // placeholder, caller re-checks
            };

            let _ = tab.close(&mut conn).await;
            pages.push(result);
        }

        Ok(pages)
    }

    /// Two-phase harvest pipeline using a single shared Chrome instance.
    ///
    /// Phase 1: search all queries. Phase 2: follow the top `max_pages`
    /// unique URLs from the aggregated search results.
    pub async fn harvest(
        &self,
        queries: &[String],
        count: usize,
        max_pages: usize,
        mut trace: Option<&mut TraceWriter>,
    ) -> Result<HarvestResult, GthingsError> {
        let pipeline_start = Instant::now();

        // Phase 1: Search all queries
        let browser = Browser::launch()
            .await
            .map_err(|e| GthingsError::Cdp(format!("Launch: {e}")))?;
        let mut conn = browser
            .connect()
            .await
            .map_err(|e| GthingsError::Cdp(format!("Connect: {e}")))?;

        if let Some(ref mut t) = trace {
            t.step(
                "session",
                0,
                "harvest",
                "launch",
                None,
                pipeline_start.elapsed().as_millis() as u64,
                None,
                None,
                None,
            );
        }

        let mut all_search_results: Vec<SearchResult> = Vec::new();

        for q in queries {
            let tab = Tab::create(&mut conn, browser.ws_url(), "about:blank")
                .await
                .map_err(|e| GthingsError::Cdp(format!("CreateTab: {e}")))?;

            let encoded = urlencoding::encode(q);
            let url = format!("https://www.google.com/search?q={}&num={}", encoded, count);

            tab.navigate(&mut conn, &url)
                .await
                .map_err(|e| GthingsError::Cdp(format!("Navigate: {e}")))?;

            tokio::time::sleep(std::time::Duration::from_secs(2)).await;

            let js = format!(
                r#"
(() => {{
    const results = [];
    const selectors = ['div.g', 'div[data-hveid]', 'div.yuRUbf'];
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
                results.push({{ title: (titleEl.innerText || '').trim(), url, snippet: (snippetEl?.innerText || '').trim() }});
            }}
        }}
    }}
    return JSON.stringify(results.slice(0, {}));
}})()
"#,
                count
            );

            if let Ok(result) = tab.evaluate(&mut conn, &js).await {
                let json_str = result["result"]["value"].as_str().unwrap_or("[]");
                let items: Vec<serde_json::Value> =
                    serde_json::from_str(json_str).unwrap_or_default();
                for item in items {
                    all_search_results.push(SearchResult {
                        title: item["title"].as_str().unwrap_or("").to_string(),
                        url: item["url"].as_str().unwrap_or("").to_string(),
                        snippet: item["snippet"].as_str().unwrap_or("").to_string(),
                        query: Some(q.to_string()),
                    });
                }
            }

            // Close the tab
            let _ = tab.close(&mut conn).await;
        }

        // Dedup and rank search results
        let mut seen = std::collections::HashSet::new();
        all_search_results.retain(|r| seen.insert(r.url.clone()));
        all_search_results.sort_by_key(|k| Reverse(k.snippet.len()));

        let unique_urls = all_search_results.len();

        // Phase 2: Follow top K URLs
        let top_urls: Vec<&SearchResult> = all_search_results.iter().take(max_pages).collect();
        let mut read_pages: Vec<FollowResult> = Vec::with_capacity(top_urls.len());
        let mut pages_skipped = 0usize;

        for sr in &top_urls {
            let tab = match Tab::create(&mut conn, browser.ws_url(), "about:blank").await {
                Ok(t) => t,
                Err(_) => {
                    pages_skipped += 1;
                    continue;
                }
            };

            if tab.navigate(&mut conn, &sr.url).await.is_err() {
                let _ = tab.close(&mut conn).await;
                pages_skipped += 1;
                continue;
            }

            tokio::time::sleep(std::time::Duration::from_secs(3)).await;

            let html = match tab.extract_html(&mut conn).await {
                Ok(h) => h,
                Err(_) => {
                    let _ = tab.close(&mut conn).await;
                    pages_skipped += 1;
                    continue;
                }
            };

            let selector = &self
                .config
                .deny_hosts
                .first()
                .map(|_| "body")
                .unwrap_or("article,main,[role=main]");
            let fallback_selector = if selector.is_empty() {
                "body"
            } else {
                selector
            };

            match extraction::html::HtmlExtractor::extract(&html, fallback_selector) {
                Ok(extracted) => {
                    let (content, truncated) = if self.config.max_chars < extracted.content.len() {
                        let end = self.config.max_chars.min(extracted.content.len());
                        (extracted.content[..end].to_string(), true)
                    } else {
                        (extracted.content, false)
                    };

                    let quality = ContentQuality::validate(&content);
                    let _ = tab.close(&mut conn).await;
                    read_pages.push(FollowResult {
                        success: true,
                        url: sr.url.clone(),
                        content: Some(content),
                        total_length: extracted.total_length,
                        offset: 0,
                        truncated,
                        sections: extracted.sections,
                        error: None,
                        quality: Some(quality),
                    });
                }
                Err(e) => {
                    let _ = tab.close(&mut conn).await;
                    pages_skipped += 1;
                    read_pages.push(FollowResult {
                        success: false,
                        url: sr.url.clone(),
                        content: None,
                        total_length: 0,
                        offset: 0,
                        truncated: false,
                        sections: Vec::new(),
                        error: Some(format!("Extract: {e}")),
                        quality: None,
                    });
                }
            }
        }

        let pages_followed = read_pages.iter().filter(|p| p.success).count();
        let total_search_results = all_search_results.len();
        let elapsed = pipeline_start.elapsed().as_millis() as u64;

        Ok(HarvestResult {
            search_results: all_search_results,
            read_pages,
            meta: HarvestMeta {
                queries: queries.to_vec(),
                total_search_results,
                unique_urls,
                pages_followed,
                pages_skipped,
                duration_ms: elapsed,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_batch_processor_creation() {
        let config = GthingsConfig::default();
        let bp = BatchProcessor::new(config);
        // Just verify it doesn't panic
        let _ = bp;
    }
}
