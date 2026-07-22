//! Batch operations — search, follow, and the two-phase harvest pipeline.
//!
//! Provides [`BatchProcessor`] which orchestrates multi-query searches,
//! multi-URL page following, and a two-phase harvest pipeline that first
//! searches all queries (dedup + rank) and then follows the top M results.
//!
//! All heavy lifting is offloaded to the gthings daemon via UDS JSON-RPC.

use std::sync::atomic::{AtomicU64, Ordering};

use common::GthingsError;
use common::config::GthingsConfig;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::types::{
    BatchSearchResult, FollowOpts, FollowResult, HarvestMeta, HarvestResult, SearchMeta,
    SearchResult,
};

/// Monotonic request ID for JSON-RPC calls to the daemon.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Send a JSON-RPC-style request to the browser daemon over UDS.
/// Returns the raw response Value (not just the result field).
/// Used by the batch RPC methods.
async fn send_request_value(method: &str, params: Option<Value>) -> Result<Value, GthingsError> {
    let socket_path = std::env::var("GTHINGS_DAEMON_SOCKET")
        .unwrap_or_else(|_| "/tmp/gthings-daemon.sock".to_string());
    let stream = UnixStream::connect(&socket_path)
        .await
        .map_err(|e| GthingsError::Other(format!("Cannot connect to daemon: {}", e)))?;

    let (reader, mut writer) = stream.into_split();
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);

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

/// Batch processor for multi-step search pipelines.
///
/// Sends all work to the gthings daemon over UDS, which manages
/// parallel browser tabs server-side.
///
/// - **`search`** — multi-query search via daemon `search.batch`.
/// - **`follow`** — multi-URL page extraction via daemon `follow.batch`.
/// - **`harvest`** — two-phase pipeline (search + follow) in one daemon call.
pub struct BatchProcessor {
    config: GthingsConfig,
}

impl BatchProcessor {
    /// Create a new [`BatchProcessor`].
    pub fn new(config: GthingsConfig) -> Self {
        Self { config }
    }

    /// Batch search: run multiple queries, deduplicate by URL, rank by
    /// snippet length descending.
    ///
    /// Delegates to the daemon's `search.batch` RPC.
    ///
    /// # Errors
    ///
    /// Returns [`GthingsError`] if the daemon call fails.
    pub async fn search(
        &self,
        queries: &[String],
        count: usize,
    ) -> Result<BatchSearchResult, GthingsError> {
        let params = serde_json::json!({
            "queries": queries,
            "count": count,
            "concurrency": self.config.search_concurrency,
            "deny_hosts": self.config.deny_hosts,
        });

        let result = send_request_value("search.batch", Some(params)).await?;

        let results: Vec<SearchResult> = result["results"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|v| SearchResult {
                        title: v["title"].as_str().unwrap_or("").to_string(),
                        url: v["url"].as_str().unwrap_or("").to_string(),
                        snippet: v["snippet"].as_str().unwrap_or("").to_string(),
                        query: v["query"].as_str().map(|s| s.to_string()),
                    })
                    .collect()
            })
            .unwrap_or_default();

        let total = results.len();

        Ok(BatchSearchResult {
            results,
            meta: SearchMeta {
                total,
                query: format!("{} queries", queries.len()),
                duration_ms: result["duration_ms"].as_u64().unwrap_or(0),
            },
        })
    }

    /// Batch follow: follow multiple URLs in parallel browser tabs.
    ///
    /// Delegates to the daemon's `follow.batch` RPC.
    ///
    /// # Errors
    ///
    /// Returns [`GthingsError`] if the daemon call fails.
    pub async fn follow(
        &self,
        urls: &[String],
        opts: FollowOpts,
    ) -> Result<Vec<FollowResult>, GthingsError> {
        let params = serde_json::json!({
            "urls": urls,
            "selector": opts.selector,
            "max_chars": opts.max_length,
            "concurrency": opts.concurrency,
        });

        let result = send_request_value("follow.batch", Some(params)).await?;

        let pages: Vec<FollowResult> = result["pages"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|v| FollowResult {
                        success: v["success"].as_bool().unwrap_or(false),
                        url: v["url"].as_str().unwrap_or("").to_string(),
                        content: v["content"].as_str().map(|s| s.to_string()),
                        total_length: v["total_length"].as_u64().unwrap_or(0) as usize,
                        offset: v["offset"].as_u64().unwrap_or(0) as usize,
                        truncated: v["truncated"].as_bool().unwrap_or(false),
                        sections: v["sections"]
                            .as_array()
                            .map(|arr| {
                                arr.iter()
                                    .map(|s| extraction::html::Section {
                                        heading: s["heading"].as_str().unwrap_or("").to_string(),
                                        content: s["content"].as_str().unwrap_or("").to_string(),
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                        error: v["error"].as_str().map(|s| s.to_string()),
                        quality: None,
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(pages)
    }

    /// Two-phase harvest pipeline — ONE daemon RPC.
    ///
    /// Sends all queries + config in a single UDS request. The daemon
    /// handles Phase 1 (parallel search) and Phase 2 (parallel follow)
    /// using its SessionPool.
    pub async fn harvest(
        &self,
        queries: &[String],
        count: usize,
        max_pages: usize,
    ) -> Result<HarvestResult, GthingsError> {
        let params = serde_json::json!({
            "queries": queries,
            "count": count,
            "follow_top_k": max_pages,
            "search_concurrency": self.config.search_concurrency,
            "follow_concurrency": self.config.follow_concurrency,
            "max_chars": self.config.max_chars,
            "deny_hosts": self.config.deny_hosts,
        });

        let result = send_request_value("harvest", Some(params)).await?;

        // Parse search_results from daemon response
        let search_results: Vec<SearchResult> = result["search_results"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|v| SearchResult {
                        title: v["title"].as_str().unwrap_or("").to_string(),
                        url: v["url"].as_str().unwrap_or("").to_string(),
                        snippet: v["snippet"].as_str().unwrap_or("").to_string(),
                        query: v["query"].as_str().map(|s| s.to_string()),
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Parse read_pages from daemon response
        let read_pages: Vec<FollowResult> = result["read_pages"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|v| FollowResult {
                        success: v["success"].as_bool().unwrap_or(false),
                        url: v["url"].as_str().unwrap_or("").to_string(),
                        content: v["content"].as_str().map(|s| s.to_string()),
                        total_length: v["total_length"].as_u64().unwrap_or(0) as usize,
                        offset: v["offset"].as_u64().unwrap_or(0) as usize,
                        truncated: v["truncated"].as_bool().unwrap_or(false),
                        sections: v["sections"]
                            .as_array()
                            .map(|arr| {
                                arr.iter()
                                    .map(|s| extraction::html::Section {
                                        heading: s["heading"].as_str().unwrap_or("").to_string(),
                                        content: s["content"].as_str().unwrap_or("").to_string(),
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                        error: v["error"].as_str().map(|s| s.to_string()),
                        quality: None,
                    })
                    .collect()
            })
            .unwrap_or_default();

        let meta = HarvestMeta {
            queries: queries.to_vec(),
            total_search_results: result["meta"]["total_search_results"].as_u64().unwrap_or(0)
                as usize,
            unique_urls: result["meta"]["unique_urls"].as_u64().unwrap_or(0) as usize,
            pages_followed: result["meta"]["pages_followed"].as_u64().unwrap_or(0) as usize,
            pages_skipped: result["meta"]["pages_skipped"].as_u64().unwrap_or(0) as usize,
            duration_ms: result["meta"]["duration_ms"].as_u64().unwrap_or(0),
        };

        Ok(HarvestResult {
            search_results,
            read_pages,
            meta,
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
