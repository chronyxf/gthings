//! Batch operations — concurrent multi-query search.
//!
//! All operations share a single [`Session`] (one WebSocket connection),
//! creating independent tabs per task with per-task timeout isolation.

use std::sync::Arc;
use std::time::Duration;

use gthings_cdp::{CdpError, Session};
use tokio::task::JoinSet;

use crate::search::SearchResult;

/// Batch processor for multi-query search pipelines.
///
/// Each operation creates one tab per query, runs them concurrently via
/// [`JoinSet`], and closes all tabs after completion.
pub struct BatchProcessor;

impl BatchProcessor {
    /// Search multiple queries concurrently.
    ///
    /// Creates one tab per query via [`Session::create_tab`], runs each
    /// search independently with a 30-second per-task timeout, and closes
    /// all tabs after completion.
    ///
    /// When `follow_results` is `true`, each search result's URL is also
    /// fetched (best-effort) to verify reachability. The followed content
    /// is not retained — use [`crate::follow::follow`] separately if
    /// content extraction is needed.
    ///
    /// # Arguments
    ///
    /// * `session` — Shared CDP session (wrapped in [`Arc`] for concurrent access).
    /// * `queries` — List of search queries.
    /// * `count` — Results per query.
    /// * `follow_results` — If `true`, attempt to load each result URL.
    /// * `follow_max_chars` — Max characters when following (ignored if `follow_results` is `false`).
    pub async fn search(
        session: Arc<Session>,
        queries: &[String],
        count: usize,
        follow_results: bool,
        follow_max_chars: usize,
    ) -> Result<Vec<Vec<SearchResult>>, CdpError> {
        let mut join_set: JoinSet<Result<Vec<SearchResult>, CdpError>> = JoinSet::new();
        let timeout = Duration::from_secs(30);

        for query in queries {
            let session = Arc::clone(&session);
            let query = query.clone();

            join_set.spawn(async move {
                // 1. Create tab outside timeout — guarantees we can close it on all paths
                let tab = match session.create_tab("about:blank").await {
                    Ok(t) => t,
                    Err(e) => return Err(e),
                };

                // 2. Search with timeout (tab is alive during this)
                let search_result = tokio::time::timeout(
                    timeout,
                    crate::search::search(&session, &tab, &query, count),
                )
                .await;

                // 3. If search succeeded and follow_results is enabled, follow best-effort
                if let Ok(Ok(ref results)) = search_result {
                    if follow_results {
                        for result in results {
                            if let Err(e) =
                                crate::follow::follow(&session, &tab, &result.url, follow_max_chars)
                                    .await
                            {
                                tracing::warn!(
                                    url = %result.url,
                                    error = %e,
                                    "batch: follow_result failed"
                                );
                            }
                        }
                    }
                }

                // 4. ALWAYS close tab — runs even on timeout or search error
                if let Err(e) = session.close_tab(tab).await {
                    tracing::warn!("close_tab failed: {e}");
                }

                // 5. Convert timeout error to CdpError
                match search_result {
                    Ok(Ok(results)) => Ok(results),
                    Ok(Err(e)) => Err(e),
                    Err(_) => Err(CdpError::CdpCallFailed {
                        method: "batch_search".into(),
                        detail: format!("timeout for query: {query}"),
                    }),
                }
            });
        }

        let mut all_results = Vec::new();
        while let Some(task) = join_set.join_next().await {
            match task {
                Ok(Ok(results)) => all_results.push(results),
                Ok(Err(e)) => return Err(e),
                Err(join_err) => {
                    return Err(CdpError::CdpCallFailed {
                        method: "batch_search".into(),
                        detail: format!("join error: {join_err}"),
                    });
                }
            }
        }
        Ok(all_results)
    }
}
