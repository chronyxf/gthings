//! Page following and content extraction.
//!
//! Provides [`PageFollower`] which fetches a URL via Chrome DevTools Protocol (CDP),
//! extracts content using CSS selectors, validates quality, and caches results locally.

use std::time::Instant;

use cdp::{Browser, Tab};

use common::GthingsError;
use common::cache::Sha256DiskCache;
use common::config::GthingsConfig;
use common::trace::TraceWriter;
use extraction::html::HtmlExtractor;
use extraction::quality::ContentQuality;

use crate::types::{FollowOpts, FollowResult};

// Helpers

/// Normalise arXiv PDF URLs to their abstract-page equivalents.
///
/// Only applies to arxiv.org (including subdomains like export.arxiv.org).
/// Transforms:
/// - `https://export.arxiv.org/pdf/2301.12345.pdf` → `https://arxiv.org/abs/2301.12345`
/// - `https://arxiv.org/pdf/2301.12345.pdf` → `https://arxiv.org/abs/2301.12345`
/// - `https://arxiv.org/pdf/2301.12345` → `https://arxiv.org/abs/2301.12345`
fn normalise_arxiv_url(url: &str) -> String {
    let url = url.trim();
    // Only apply to arxiv.org (and subdomains like export.arxiv.org)
    if !url.contains("arxiv.org") {
        return url.to_string();
    }
    // export.arxiv.org -> arxiv.org
    let url = url.replace("export.arxiv.org", "arxiv.org");
    // /pdf/ -> /abs/
    let url = url.replace("/pdf/", "/abs/");
    // Strip .pdf suffix
    if let Some(stripped) = url.strip_suffix(".pdf") {
        return stripped.to_string();
    }
    url
}

// PageFollower

/// Page follower with caching and quality validation.
///
/// Fetches web pages via an ephemeral Chrome CDP instance, extracts structured
/// content using CSS selectors via [`HtmlExtractor`], checks quality, and
/// persists results in a disk cache.
pub struct PageFollower {
    cache: Sha256DiskCache,
}

impl PageFollower {
    /// Create a new [`PageFollower`].
    pub fn new(config: GthingsConfig) -> Self {
        let cache = Sha256DiskCache::new(&config.cache_dir, config.cache_ttl_secs);
        Self { cache }
    }

    /// Follow a single URL and extract page content.
    ///
    /// Checks the disk cache before fetching. On success, writes the
    /// result to cache. Runs content quality validation on the extracted
    /// text and attaches the [`QualityResult`](extraction::quality::QualityResult)
    /// to the returned [`FollowResult`].
    ///
    /// # arXiv URLs
    ///
    /// PDF URLs (`/pdf/…` or ending in `.pdf`) are automatically rewritten
    /// to their abstract-page equivalents (`/abs/…`).
    ///
    /// # Errors
    ///
    /// Returns [`GthingsError::Cdp`] if Chrome cannot be launched or the
    /// CDP call fails, or [`GthingsError::Parse`] if content extraction fails.
    pub async fn follow(
        &self,
        url: &str,
        opts: FollowOpts,
        trace: Option<&mut TraceWriter>,
    ) -> Result<FollowResult, GthingsError> {
        let start = Instant::now();

        // 1. Normalise arXiv URLs
        let normalised = normalise_arxiv_url(url);

        // 2. Check cache (uses FollowResult for validation)
        let cache_key = self.cache.key(&normalised, opts.offset, opts.max_length);
        if let Some(cached_json) = self.check_cache(&cache_key) {
            tracing::debug!(url = %normalised, "follow: cache hit");
            let mut result = Self::parse_follow_result_json(&cached_json)?;
            result.quality = Some(ContentQuality::validate(
                result.content.as_deref().unwrap_or(""),
            ));
            return Ok(result);
        }

        // 3. Fetch page content (with retry on low quality)
        let result = self.follow_inner(&normalised, &opts, trace).await?;

        // 4. Cache the result
        if result.content.is_some() {
            if let Ok(json) = serde_json::to_string(&result) {
                self.cache.set(&cache_key, &json);
            }
        }

        let elapsed = start.elapsed().as_millis() as u64;
        tracing::debug!(
            url = %normalised,
            success = result.success,
            len = result.content.as_ref().map(|c| c.len()).unwrap_or(0),
            elapsed_ms = elapsed,
            "follow: done"
        );

        Ok(result)
    }

    /// Inner fetch-and-validate with optional retry.
    async fn follow_inner(
        &self,
        url: &str,
        opts: &FollowOpts,
        mut trace: Option<&mut TraceWriter>,
    ) -> Result<FollowResult, GthingsError> {
        // Step 1: Browser launch / reuse
        let browser_start = Instant::now();
        let browser = Browser::launch()
            .await
            .map_err(|e| GthingsError::Cdp(format!("Launch: {e}")))?;
        if let Some(ref mut t) = trace {
            t.step(
                "session",
                1,
                "follow",
                "browser_reuse",
                None,
                browser_start.elapsed().as_millis() as u64,
                None,
                None,
                None,
            );
        }

        let mut conn = browser
            .connect()
            .await
            .map_err(|e| GthingsError::Cdp(format!("Connect: {e}")))?;

        // Step 2: Create tab and navigate
        let tab_start = Instant::now();
        let tab = Tab::create(&mut conn, browser.ws_url(), "about:blank")
            .await
            .map_err(|e| GthingsError::Cdp(format!("CreateTab: {e}")))?;
        if let Some(ref mut t) = trace {
            t.step(
                "session",
                2,
                "follow",
                "tab_create",
                None,
                tab_start.elapsed().as_millis() as u64,
                None,
                None,
                None,
            );
        }

        // Step 3: Navigate
        let nav_start = Instant::now();
        tab.navigate(&mut conn, url)
            .await
            .map_err(|e| GthingsError::Cdp(format!("Navigate: {e}")))?;
        if let Some(ref mut t) = trace {
            t.step(
                "session",
                3,
                "follow",
                "navigate",
                Some(url),
                nav_start.elapsed().as_millis() as u64,
                Some(serde_json::json!({"url": url})),
                None,
                None,
            );
        }

        // Wait for JS rendering
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

        // 3c. Extract title
        let _title = tab
            .extract_title(&mut conn)
            .await
            .map_err(|e| GthingsError::Cdp(format!("Title: {e}")))?;

        // 3d. Extract HTML and use HtmlExtractor for content + sections
        let html = tab
            .extract_html(&mut conn)
            .await
            .map_err(|e| GthingsError::Cdp(format!("Html: {e}")))?;

        let selector = if opts.selector.is_empty() {
            "body"
        } else {
            &opts.selector
        };

        let extracted = HtmlExtractor::extract(&html, selector)
            .map_err(|e| GthingsError::Parse(format!("Extraction failed: {e}")))?;

        let full_text = extracted.content;
        let total_length = extracted.total_length;
        let sections = extracted.sections;

        // 3e. Apply offset and max_length truncation
        let (content, truncated) = if opts.offset > 0 || opts.max_length < full_text.len() {
            let start = opts.offset.min(full_text.len());
            let end = (start + opts.max_length).min(full_text.len());
            let is_truncated = end < total_length || opts.offset > 0;
            (full_text[start..end].to_string(), is_truncated)
        } else {
            (full_text, false)
        };

        let mut result = FollowResult {
            success: !content.is_empty(),
            url: url.to_string(),
            content: Some(content.clone()),
            total_length,
            offset: opts.offset,
            truncated,
            sections,
            error: None,
            quality: None,
        };

        // 3f. Quality gate
        let quality = ContentQuality::validate(&content);
        result.quality = Some(quality.clone());

        // Trace: extraction result
        if let Some(ref mut t) = trace {
            t.step(
                "session",
                4,
                "follow",
                "extract",
                Some(url),
                0, // duration accounted per-step
                None,
                Some(serde_json::json!({
                    "content_length": total_length,
                    "truncated": truncated,
                    "quality": {"is_ok": quality.is_ok, "score": quality.score}
                })),
                None,
            );
        }

        // 3g. Retry on low quality with body selector fallback
        if !quality.is_ok && opts.retry_on_low_quality && ContentQuality::needs_recrawl(&quality) {
            tracing::debug!(
                url = %url,
                score = quality.score,
                reasons = ?quality.reasons,
                "follow: low quality, retrying with body selector"
            );

            // Re-extract with body selector (tab still on the page)
            let retry_html = tab
                .extract_html(&mut conn)
                .await
                .map_err(|e| GthingsError::Cdp(format!("Html retry: {e}")))?;

            if let Ok(retry_extracted) = HtmlExtractor::extract(&retry_html, "body") {
                let retry_full_text = retry_extracted.content;
                let retry_total_length = retry_extracted.total_length;
                let retry_sections = retry_extracted.sections;

                let (retry_content, retry_truncated) =
                    if opts.offset > 0 || opts.max_length < retry_full_text.len() {
                        let start = opts.offset.min(retry_full_text.len());
                        let end = (start + opts.max_length).min(retry_full_text.len());
                        (retry_full_text[start..end].to_string(), true)
                    } else {
                        (retry_full_text, false)
                    };

                let retry_quality = ContentQuality::validate(&retry_content);
                if retry_quality.is_ok {
                    let _ = tab.close(&mut conn).await;
                    return Ok(FollowResult {
                        success: true,
                        url: url.to_string(),
                        content: Some(retry_content),
                        total_length: retry_total_length,
                        offset: opts.offset,
                        truncated: retry_truncated,
                        sections: retry_sections,
                        error: None,
                        quality: Some(retry_quality),
                    });
                }
            }
        }

        // Close the tab
        let close_start = Instant::now();
        let _ = tab.close(&mut conn).await;
        if let Some(ref mut t) = trace {
            t.step(
                "session",
                5,
                "follow",
                "tab_close",
                None,
                close_start.elapsed().as_millis() as u64,
                None,
                None,
                None,
            );
        }

        Ok(result)
    }

    /// Batch follow multiple URLs sequentially.
    ///
    /// Each URL goes through the same cache/CDP/quality pipeline as
    /// [`follow`](PageFollower::follow).
    ///
    /// # Errors
    ///
    /// Returns [`GthingsError`] on the first failure.
    pub async fn batch(
        &self,
        urls: &[String],
        opts: FollowOpts,
        mut trace: Option<&mut TraceWriter>,
    ) -> Result<Vec<FollowResult>, GthingsError> {
        if urls.is_empty() {
            return Ok(Vec::new());
        }

        let start = Instant::now();
        let mut results = Vec::with_capacity(urls.len());

        for url in urls {
            let result = self.follow(url, opts.clone(), trace.as_deref_mut()).await?;
            results.push(result);
        }

        let elapsed = start.elapsed().as_millis() as u64;
        tracing::debug!(
            n_results = results.len(),
            elapsed_ms = elapsed,
            "batch-follow: done"
        );

        Ok(results)
    }

    // Private helpers

    /// Check the disk cache for a previously stored result.
    fn check_cache(&self, key: &str) -> Option<String> {
        match self.cache.get(key) {
            Ok(Some(data)) => {
                // Validate that the cached JSON can be parsed as a FollowResult.
                if serde_json::from_str::<FollowResult>(&data).is_ok() {
                    return Some(data);
                }
                tracing::debug!("cache: stale/invalid entry, ignoring");
                None
            }
            Ok(None) => None,
            Err(e) => {
                tracing::debug!("cache read error: {e}");
                None
            }
        }
    }

    /// Parse a cached JSON string directly into a [`FollowResult`].
    fn parse_follow_result_json(json: &str) -> Result<FollowResult, GthingsError> {
        serde_json::from_str::<FollowResult>(json).map_err(|e| {
            GthingsError::Parse(format!(
                "failed to parse cached FollowResult JSON: {e} (len={})",
                json.len()
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalise_arxiv_pdf_slash() {
        let url = "https://arxiv.org/pdf/2301.12345.pdf";
        assert_eq!(normalise_arxiv_url(url), "https://arxiv.org/abs/2301.12345");
    }

    #[test]
    fn test_normalise_arxiv_pdf_no_ext() {
        let url = "https://arxiv.org/pdf/2301.12345";
        assert_eq!(normalise_arxiv_url(url), "https://arxiv.org/abs/2301.12345");
    }

    #[test]
    fn test_normalise_non_arxiv_unchanged() {
        // Non-arxiv URLs (even with .pdf) should be left alone
        let url = "https://example.com/paper.pdf";
        assert_eq!(normalise_arxiv_url(url), url);
    }

    #[test]
    fn test_normalise_arxiv_export() {
        let url = "https://export.arxiv.org/pdf/2301.12345.pdf";
        assert_eq!(normalise_arxiv_url(url), "https://arxiv.org/abs/2301.12345");
    }

    #[test]
    fn test_normalise_arxiv_abs_left_alone() {
        let url = "https://arxiv.org/abs/2301.12345";
        assert_eq!(normalise_arxiv_url(url), url);
    }
}
