//! Page following and content extraction.
//!
//! Provides [`PageFollower`] which fetches a URL via the browser daemon
//! (UDS → daemon → cdp-core → Chrome CDP), extracts content using CSS
//! selectors, validates quality, and caches results locally.
//!
//! The daemon call replaces the previous Phase‑1 subprocess approach
//! (`bun skills/cdp/scripts/browser-automation.ts`).

use std::time::Instant;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use common::GthingsError;
use common::cache::Sha256DiskCache;
use common::config::GthingsConfig;
use extraction::html::HtmlExtractor;
use extraction::quality::ContentQuality;

use crate::types::{FollowOpts, FollowResult};

/// Default daemon socket path.
const DAEMON_SOCKET: &str = "/tmp/gthings-daemon.sock";
static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Send a JSON-RPC-style request to the browser daemon over UDS.
async fn send_request(method: &str, params: Option<Value>) -> Result<Value, GthingsError> {
    let socket_path =
        std::env::var("GTHINGS_DAEMON_SOCKET").unwrap_or_else(|_| DAEMON_SOCKET.to_string());
    let stream = UnixStream::connect(&socket_path)
        .await
        .map_err(|e| GthingsError::Other(format!("Cannot connect to daemon: {}", e)))?;

    let (reader, mut writer) = stream.into_split();
    let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    let request = serde_json::json!({"id": id, "method": method, "params": params});
    let mut buf = serde_json::to_vec(&request).map_err(|e| GthingsError::Parse(e.to_string()))?;
    buf.push(b'\n');
    writer.write_all(&buf).await.map_err(GthingsError::Io)?;
    writer.shutdown().await.map_err(GthingsError::Io)?;

    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .await
        .map_err(GthingsError::Io)?;

    let response: Value =
        serde_json::from_str(&line).map_err(|e| GthingsError::Parse(e.to_string()))?;
    if response["ok"].as_bool().unwrap_or(false) {
        Ok(response["result"].clone())
    } else {
        Err(GthingsError::Other(
            response["error"]
                .as_str()
                .unwrap_or("unknown daemon error")
                .to_string(),
        ))
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────

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

// ── PageFollower ──────────────────────────────────────────────────────────

/// Page follower with caching and quality validation.
///
/// Fetches web pages through the browser daemon over UDS, extracts structured
/// content, checks quality, and persists results in a disk cache.
pub struct PageFollower {
    #[allow(dead_code)]
    config: GthingsConfig,
    cache: Sha256DiskCache,
    #[allow(dead_code)]
    quality: ContentQuality,
    #[allow(dead_code)]
    html_extractor: HtmlExtractor,
}

impl PageFollower {
    /// Create a new [`PageFollower`].
    pub fn new(config: GthingsConfig) -> Self {
        let cache = Sha256DiskCache::new(&config.cache_dir, config.cache_ttl_secs);
        Self {
            config,
            cache,
            quality: ContentQuality,
            html_extractor: HtmlExtractor,
        }
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
    /// Returns [`GthingsError::Other`] if the daemon cannot be reached,
    /// or [`GthingsError::Parse`] if the response is malformed.
    pub async fn follow(&self, url: &str, opts: FollowOpts) -> Result<FollowResult, GthingsError> {
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

        // 3. Fetch via daemon (with retry on low quality)
        let result = self.follow_inner(&normalised, &opts).await?;

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
    ) -> Result<FollowResult, GthingsError> {
        // 3a. Fetch content from the daemon
        let daemon_result = send_request(
            "follow",
            Some(serde_json::json!({
                "url": url,
                "selector": opts.selector,
                "offset": opts.offset,
                "max_length": opts.max_length,
            })),
        )
        .await?;

        // 3b. Build FollowResult from daemon response
        let content = daemon_result["content"].as_str().unwrap_or("").to_string();
        let total_length = daemon_result["total_length"].as_u64().unwrap_or(0) as usize;
        let truncated = daemon_result["truncated"].as_bool().unwrap_or(false);

        // Parse sections from daemon response
        let sections: Vec<extraction::html::Section> = daemon_result["sections"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|v| extraction::html::Section {
                        heading: v["heading"].as_str().unwrap_or("").to_string(),
                        content: v["content"].as_str().unwrap_or("").to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default();

        let mut result = FollowResult {
            success: !content.is_empty(),
            url: url.to_string(),
            content: Some(content.clone()),
            total_length,
            offset: opts.offset,
            truncated,
            sections, // parsed from daemon response
            error: None,
            quality: None,
        };

        // 3c. Quality gate
        let quality = ContentQuality::validate(&content);
        result.quality = Some(quality.clone());

        if !quality.is_ok && opts.retry_on_low_quality && ContentQuality::needs_recrawl(&quality) {
            tracing::debug!(
                url = %url,
                score = quality.score,
                reasons = ?quality.reasons,
                "follow: low quality, retrying with fallback selector"
            );
            let retry_opts = FollowOpts {
                selector: "body".into(),
                timeout_ms: opts.timeout_ms.max(30000),
                ..opts.clone()
            };
            if let Ok(retry) = send_request(
                "follow",
                Some(serde_json::json!({
                    "url": url,
                    "selector": retry_opts.selector,
                    "offset": retry_opts.offset,
                    "max_length": retry_opts.max_length,
                })),
            )
            .await
            {
                let retry_content = retry["content"].as_str().unwrap_or("").to_string();
                let retry_total_length = retry["total_length"].as_u64().unwrap_or(0) as usize;
                let retry_truncated = retry["truncated"].as_bool().unwrap_or(false);
                let retry_quality = ContentQuality::validate(&retry_content);
                if retry_quality.is_ok {
                    // Parse sections from retry response
                    let retry_sections: Vec<extraction::html::Section> = retry["sections"]
                        .as_array()
                        .map(|arr| {
                            arr.iter()
                                .map(|v| extraction::html::Section {
                                    heading: v["heading"].as_str().unwrap_or("").to_string(),
                                    content: v["content"].as_str().unwrap_or("").to_string(),
                                })
                                .collect()
                        })
                        .unwrap_or_default();

                    return Ok(FollowResult {
                        success: true,
                        url: url.to_string(),
                        content: Some(retry_content),
                        total_length: retry_total_length,
                        offset: retry_opts.offset,
                        truncated: retry_truncated,
                        sections: retry_sections,
                        error: None,
                        quality: Some(retry_quality),
                    });
                }
            }
        }

        Ok(result)
    }

    /// Batch follow multiple URLs sequentially.
    ///
    /// Each URL goes through the same cache/daemon/quality pipeline as
    /// [`follow`](PageFollower::follow).
    ///
    /// # Errors
    ///
    /// Returns [`GthingsError`] on the first failure.
    pub async fn batch(
        &self,
        urls: &[String],
        opts: FollowOpts,
    ) -> Result<Vec<FollowResult>, GthingsError> {
        if urls.is_empty() {
            return Ok(Vec::new());
        }

        let start = Instant::now();
        let mut results = Vec::with_capacity(urls.len());

        for url in urls {
            let result = self.follow(url, opts.clone()).await?;
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

    // ── Private helpers ──────────────────────────────────────────────────

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
