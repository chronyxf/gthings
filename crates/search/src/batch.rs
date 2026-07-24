//! Batch operations — search, follow, and the two-phase harvest pipeline.
//!
//! Provides [`BatchProcessor`] which orchestrates multi-query searches,
//! multi-URL page following, and a two-phase harvest pipeline.
//! Each operation launches a single Chrome instance and reuses it across
//! all queries/URLs in the batch.

use std::cmp::Reverse;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cdp::{Browser, Connection, Tab};

use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use common::GthingsError;
use common::config::GthingsConfig;
use common::trace::TraceWriter;
use extraction::quality::ContentQuality;

use crate::types::{
    BatchSearchResult, FollowOpts, FollowResult, HarvestMeta, HarvestResult, SearchMeta,
    SearchResult,
};

/// Wait for page load by polling `document.readyState` at 100ms intervals.
/// Returns when `"complete"` plus a 200ms rendering buffer, or on timeout.
async fn wait_for_page_load(
    tab: &Tab,
    conn: &mut Connection,
    timeout: Duration,
) -> Result<(), cdp::error::CdpError> {
    let start = Instant::now();
    loop {
        let result = tab.evaluate(conn, "document.readyState").await?;
        let ready = result["result"]["value"].as_str();
        if ready == Some("complete") {
            // Extra wait for JS rendering to settle
            tokio::time::sleep(Duration::from_millis(200)).await;
            return Ok(());
        }
        if start.elapsed() > timeout {
            return Ok(()); // proceed anyway on timeout
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Batch processor for multi-step search pipelines.
///
/// Each operation reuses a single Chrome instance across the batch.
pub struct BatchProcessor {
    config: GthingsConfig,
}

impl BatchProcessor {
    /// Create a new [`BatchProcessor`].
    pub fn new(config: GthingsConfig) -> Self {
        Self { config }
    }

    /// Batch search: run multiple queries with one shared Chrome instance,
    /// deduplicate by URL, rank by snippet length descending.
    ///
    /// Queries are processed concurrently, bounded by [`GthingsConfig::search_concurrency`].
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

        let browser = Arc::new(
            Browser::launch()
                .await
                .map_err(|e| GthingsError::Cdp(format!("Launch: {e}")))?,
        );
        let ws_url = browser.ws_url().to_string();

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

        let concurrency_limit = self.config.search_concurrency.max(1);
        let semaphore = Arc::new(Semaphore::new(concurrency_limit));
        let mut join_set: JoinSet<Result<Vec<SearchResult>, GthingsError>> = JoinSet::new();

        for q in queries {
            let permit = semaphore
                .clone()
                .acquire_owned()
                .await
                .map_err(|e| GthingsError::Cdp(format!("Semaphore: {e}")))?;
            let browser = browser.clone();
            let ws_url = ws_url.clone();
            let q = q.clone();
            let encoded = urlencoding::encode(&q);
            let url = format!("https://www.google.com/search?q={}&num={}", encoded, count);
            let count_val = count;
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
                count_val
            );

            join_set.spawn(async move {
                let _permit = permit;
                let mut conn = browser
                    .connect()
                    .await
                    .map_err(|e| GthingsError::Cdp(format!("Connect: {e}")))?;
                let tab = Tab::create(&mut conn, &ws_url, "about:blank")
                    .await
                    .map_err(|e| GthingsError::Cdp(format!("CreateTab: {e}")))?;

                tab.navigate(&mut conn, &url)
                    .await
                    .map_err(|e| GthingsError::Cdp(format!("Navigate: {e}")))?;

                wait_for_page_load(&tab, &mut conn, Duration::from_secs(10))
                    .await
                    .map_err(|e| GthingsError::Cdp(format!("WaitLoad: {e}")))?;

                let result = tab
                    .evaluate(&mut conn, &js)
                    .await
                    .map_err(|e| GthingsError::Cdp(format!("Eval: {e}")))?;

                let json_str = result["result"]["value"].as_str().unwrap_or("[]");
                let items: Vec<serde_json::Value> =
                    serde_json::from_str(json_str).unwrap_or_default();

                let mut results = Vec::with_capacity(items.len());
                for item in items {
                    let mut result: SearchResult = serde_json::from_value(item).unwrap_or_default();
                    result.query = Some(q.clone());
                    results.push(result);
                }

                let _ = tab.close(&mut conn).await;

                Ok(results)
            });
        }

        let mut all_results: Vec<SearchResult> = Vec::new();
        while let Some(task_result) = join_set.join_next().await {
            match task_result {
                Ok(Ok(results)) => all_results.extend(results),
                Ok(Err(e)) => return Err(e),
                Err(join_err) => {
                    return Err(GthingsError::Cdp(format!("Task join failed: {join_err}")));
                }
            }
        }

        // Dedup by URL
        let mut seen = std::collections::HashSet::new();
        all_results.retain(|r| seen.insert(r.url.clone()));

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
    /// URLs are processed concurrently, bounded by [`GthingsConfig::follow_concurrency`].
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

        let browser = Arc::new(
            Browser::launch()
                .await
                .map_err(|e| GthingsError::Cdp(format!("Launch: {e}")))?,
        );
        let ws_url = browser.ws_url().to_string();

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

        let concurrency_limit = self.config.follow_concurrency.max(1);
        let semaphore = Arc::new(Semaphore::new(concurrency_limit));
        let mut join_set: JoinSet<Result<FollowResult, GthingsError>> = JoinSet::new();

        for url in urls {
            let permit = semaphore
                .clone()
                .acquire_owned()
                .await
                .map_err(|e| GthingsError::Cdp(format!("Semaphore: {e}")))?;
            let browser = browser.clone();
            let ws_url = ws_url.clone();
            let url = url.clone();
            let opts = opts.clone();

            join_set.spawn(async move {
                let _permit = permit;
                let mut conn = browser
                    .connect()
                    .await
                    .map_err(|e| GthingsError::Cdp(format!("Connect: {e}")))?;
                let tab = Tab::create(&mut conn, &ws_url, "about:blank")
                    .await
                    .map_err(|e| GthingsError::Cdp(format!("CreateTab: {e}")))?;

                tab.navigate(&mut conn, &url)
                    .await
                    .map_err(|e| GthingsError::Cdp(format!("Navigate: {e}")))?;

                wait_for_page_load(&tab, &mut conn, Duration::from_millis(opts.timeout_ms))
                    .await
                    .map_err(|e| GthingsError::Cdp(format!("WaitLoad: {e}")))?;

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
                        return Ok(FollowResult {
                            url,
                            content: None,
                            total_length: 0,
                            offset: opts.offset,
                            sections: Vec::new(),
                            error: Some(format!("Extract: {e}")),
                            quality: None,
                            success: false,
                            truncated: false,
                        });
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

                let is_success = !content.is_empty();
                let result = FollowResult {
                    url,
                    content: Some(content),
                    total_length: extracted.total_length,
                    offset: opts.offset,
                    sections: extracted.sections,
                    error: None,
                    quality: Some(ContentQuality::validate("")), // placeholder, caller re-checks
                    success: is_success,
                    truncated,
                };

                let _ = tab.close(&mut conn).await;
                Ok(result)
            });
        }

        let mut pages: Vec<FollowResult> = Vec::with_capacity(urls.len());
        while let Some(task_result) = join_set.join_next().await {
            match task_result {
                Ok(Ok(result)) => pages.push(result),
                Ok(Err(e)) => return Err(e),
                Err(join_err) => {
                    return Err(GthingsError::Cdp(format!("Task join failed: {join_err}")));
                }
            }
        }

        Ok(pages)
    }

    /// Two-phase harvest pipeline using a single shared Chrome instance.
    ///
    /// Phase 1 (search) runs with [`GthingsConfig::search_concurrency`];
    /// Phase 2 (follow) runs with [`GthingsConfig::follow_concurrency`].
    pub async fn harvest(
        &self,
        queries: &[String],
        count: usize,
        max_pages: usize,
        mut trace: Option<&mut TraceWriter>,
    ) -> Result<HarvestResult, GthingsError> {
        let pipeline_start = Instant::now();

        // Phase 1: Search all queries (parallel)
        let browser = Arc::new(
            Browser::launch()
                .await
                .map_err(|e| GthingsError::Cdp(format!("Launch: {e}")))?,
        );
        let ws_url = browser.ws_url().to_string();

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

        let search_concurrency = self.config.search_concurrency.max(1);
        let semaphore = Arc::new(Semaphore::new(search_concurrency));
        let mut search_set: JoinSet<Result<Vec<SearchResult>, GthingsError>> = JoinSet::new();

        for q in queries {
            let permit = semaphore
                .clone()
                .acquire_owned()
                .await
                .map_err(|e| GthingsError::Cdp(format!("Semaphore: {e}")))?;
            let browser = browser.clone();
            let ws_url = ws_url.clone();
            let q = q.clone();
            let encoded = urlencoding::encode(&q);
            let url = format!("https://www.google.com/search?q={}&num={}", encoded, count);
            let count_val = count;
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
                count_val
            );

            search_set.spawn(async move {
                let _permit = permit;
                let mut conn = browser
                    .connect()
                    .await
                    .map_err(|e| GthingsError::Cdp(format!("Connect: {e}")))?;
                let tab = Tab::create(&mut conn, &ws_url, "about:blank")
                    .await
                    .map_err(|e| GthingsError::Cdp(format!("CreateTab: {e}")))?;

                if tab.navigate(&mut conn, &url).await.is_err() {
                    let _ = tab.close(&mut conn).await;
                    return Ok(Vec::new());
                }

                let _ = wait_for_page_load(&tab, &mut conn, Duration::from_secs(10)).await;

                let mut results = Vec::new();
                if let Ok(result) = tab.evaluate(&mut conn, &js).await {
                    let json_str = result["result"]["value"].as_str().unwrap_or("[]");
                    let items: Vec<serde_json::Value> =
                        serde_json::from_str(json_str).unwrap_or_default();
                    for item in items {
                        let mut result: SearchResult =
                            serde_json::from_value(item).unwrap_or_default();
                        result.query = Some(q.clone());
                        results.push(result);
                    }
                }

                let _ = tab.close(&mut conn).await;
                Ok(results)
            });
        }

        let mut all_search_results: Vec<SearchResult> = Vec::new();
        while let Some(task_result) = search_set.join_next().await {
            match task_result {
                Ok(Ok(results)) => all_search_results.extend(results),
                Ok(Err(e)) => return Err(e),
                Err(join_err) => {
                    return Err(GthingsError::Cdp(format!(
                        "Search task join failed: {join_err}"
                    )));
                }
            }
        }

        // Dedup and rank search results
        let mut seen = std::collections::HashSet::new();
        all_search_results.retain(|r| seen.insert(r.url.clone()));
        all_search_results.sort_by_key(|k| Reverse(k.snippet.len()));

        let unique_urls = all_search_results.len();

        let follow_concurrency = self.config.follow_concurrency.max(1);
        let semaphore = Arc::new(Semaphore::new(follow_concurrency));
        let mut follow_set: JoinSet<Result<FollowResult, GthingsError>> = JoinSet::new();

        let top_urls: Vec<&SearchResult> = all_search_results.iter().take(max_pages).collect();
        let max_chars = self.config.max_chars;

        for sr in &top_urls {
            let permit = semaphore
                .clone()
                .acquire_owned()
                .await
                .map_err(|e| GthingsError::Cdp(format!("Semaphore: {e}")))?;
            let browser = browser.clone();
            let ws_url = ws_url.clone();
            let url = sr.url.clone();
            follow_set.spawn(async move {
                let _permit = permit;
                let mut conn = match browser.connect().await {
                    Ok(c) => c,
                    Err(e) => {
                        return Ok(FollowResult {
                            url,
                            content: None,
                            total_length: 0,
                            offset: 0,
                            sections: Vec::new(),
                            error: Some(format!("Connect: {e}")),
                            quality: None,
                            success: false,
                            truncated: false,
                        });
                    }
                };

                let tab = match Tab::create(&mut conn, &ws_url, "about:blank").await {
                    Ok(t) => t,
                    Err(_) => {
                        return Ok(FollowResult {
                            url,
                            content: None,
                            total_length: 0,
                            offset: 0,
                            sections: Vec::new(),
                            error: Some("CreateTab failed".into()),
                            quality: None,
                            success: false,
                            truncated: false,
                        });
                    }
                };

                if tab.navigate(&mut conn, &url).await.is_err() {
                    let _ = tab.close(&mut conn).await;
                    return Ok(FollowResult {
                        url,
                        content: None,
                        total_length: 0,
                        offset: 0,
                        sections: Vec::new(),
                        error: Some("Navigate failed".into()),
                        quality: None,
                        success: false,
                        truncated: false,
                    });
                }

                let _ = wait_for_page_load(&tab, &mut conn, Duration::from_secs(15)).await;

                let html = match tab.extract_html(&mut conn).await {
                    Ok(h) => h,
                    Err(_) => {
                        let _ = tab.close(&mut conn).await;
                        return Ok(FollowResult {
                            url,
                            content: None,
                            total_length: 0,
                            offset: 0,
                            sections: Vec::new(),
                            error: Some("ExtractHtml failed".into()),
                            quality: None,
                            success: false,
                            truncated: false,
                        });
                    }
                };

                let fallback_selector = "article,main,[role=main]";

                match extraction::html::HtmlExtractor::extract(&html, fallback_selector) {
                    Ok(extracted) => {
                        let (content, truncated) = if max_chars < extracted.content.len() {
                            let end = max_chars.min(extracted.content.len());
                            (extracted.content[..end].to_string(), true)
                        } else {
                            (extracted.content, false)
                        };

                        let quality = ContentQuality::validate(&content);
                        let _ = tab.close(&mut conn).await;
                        Ok(FollowResult {
                            url,
                            content: Some(content),
                            total_length: extracted.total_length,
                            offset: 0,
                            sections: extracted.sections,
                            error: None,
                            quality: Some(quality),
                            success: true,
                            truncated,
                        })
                    }
                    Err(e) => {
                        let _ = tab.close(&mut conn).await;
                        Ok(FollowResult {
                            url,
                            content: None,
                            total_length: 0,
                            offset: 0,
                            sections: Vec::new(),
                            error: Some(format!("Extract: {e}")),
                            quality: None,
                            success: false,
                            truncated: false,
                        })
                    }
                }
            });
        }

        let mut read_pages: Vec<FollowResult> = Vec::with_capacity(top_urls.len());
        let mut pages_skipped = 0usize;

        while let Some(task_result) = follow_set.join_next().await {
            match task_result {
                Ok(Ok(result)) => {
                    if !result.success {
                        pages_skipped += 1;
                    }
                    read_pages.push(result);
                }
                Ok(Err(e)) => return Err(e),
                Err(join_err) => {
                    return Err(GthingsError::Cdp(format!(
                        "Follow task join failed: {join_err}"
                    )));
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
        let _ = bp;
    }
}
