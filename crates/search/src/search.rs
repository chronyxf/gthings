//! Google search implementation.
//!
//! Provides [`GoogleSearch`] for executing web searches via Google.
//! Search requests are sent to the browser daemon over UDS, which
//! proxies them through cdp-core to Chrome CDP.

use std::time::Instant;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use common::GthingsError;
use common::config::GthingsConfig;

use crate::types::{BatchSearchResult, SearchMeta, SearchResult};

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

// ── GoogleSearch ──────────────────────────────────────────────────────────

/// Google web search client.
///
/// Search queries are dispatched via UDS to the browser daemon, which
/// navigates a Chrome tab to google.com/search, extracts organic results
/// via CDP, and returns structured JSON.
pub struct GoogleSearch {
    #[allow(dead_code)]
    client: reqwest::Client,
    #[allow(dead_code)]
    config: GthingsConfig,
}

impl GoogleSearch {
    /// Create a new [`GoogleSearch`] instance.
    pub fn new(config: GthingsConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(config.request_timeout_ms))
            .build()
            .unwrap_or_default();
        Self { client, config }
    }

    /// Execute a single Google search query.
    ///
    /// Sends a `search` request to the browser daemon over UDS.
    /// Returns a list of [`SearchResult`]s extracted from the SERP.
    ///
    /// # Errors
    ///
    /// Returns [`GthingsError::Other`] if the daemon cannot be reached
    /// or returns an error response.
    pub async fn query(&self, q: &str, count: usize) -> Result<Vec<SearchResult>, GthingsError> {
        let start = Instant::now();

        let result = send_request(
            "search",
            Some(serde_json::json!({
                "query": q,
                "count": count,
            })),
        )
        .await?;

        let items = result["results"].as_array().cloned().unwrap_or_default();
        let mut search_results: Vec<SearchResult> = items
            .iter()
            .map(|item| SearchResult {
                title: item["title"].as_str().unwrap_or("").to_string(),
                url: item["url"].as_str().unwrap_or("").to_string(),
                snippet: item["snippet"].as_str().unwrap_or("").to_string(),
                query: Some(q.to_string()),
            })
            .collect();

        // Empty-result retry: append a trailing space and retry once
        if search_results.is_empty() && !q.ends_with(' ') {
            let retry_q = format!("{} ", q);
            tracing::debug!(
                query = q,
                "search: empty result, retrying with trailing space"
            );
            if let Ok(retry) = send_request(
                "search",
                Some(serde_json::json!({
                    "query": retry_q,
                    "count": count,
                })),
            )
            .await
            {
                if let Some(retry_items) = retry["results"].as_array() {
                    if !retry_items.is_empty() {
                        search_results = retry_items
                            .iter()
                            .map(|item| SearchResult {
                                title: item["title"].as_str().unwrap_or("").to_string(),
                                url: item["url"].as_str().unwrap_or("").to_string(),
                                snippet: item["snippet"].as_str().unwrap_or("").to_string(),
                                query: Some(q.to_string()),
                            })
                            .collect();
                    }
                }
            }
        }

        let elapsed = start.elapsed().as_millis() as u64;
        tracing::debug!(
            query = q,
            count = search_results.len(),
            elapsed_ms = elapsed,
            "search: done"
        );

        Ok(search_results)
    }

    /// Batch search multiple queries.
    ///
    /// Each query is sent individually to the daemon. Results are
    /// deduplicated by URL and ranked by snippet length (descending).
    ///
    /// # Errors
    ///
    /// Returns [`GthingsError`] if any daemon request fails.
    pub async fn batch(
        &self,
        queries: &[String],
        count: usize,
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
            let mut results = self.query(q, count).await?;
            all_results.append(&mut results);
        }

        // Dedup by URL
        let mut seen = std::collections::HashSet::new();
        all_results.retain(|r| seen.insert(r.url.clone()));

        // Sort by snippet length descending
        all_results.sort_by(|a, b| b.snippet.len().cmp(&a.snippet.len()));

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
