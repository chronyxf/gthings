//! Batch operations — concurrent multi-query search.
//!
//! All operations share a single [`Session`] (one WebSocket connection),
//! creating independent tabs per task with per-task timeout isolation.

use std::sync::Arc;
use std::time::Duration;

use gthings_cdp::{CdpError, Session};
use gthings_common::domain_reputation::DomainReputation;
use gthings_common::pagination::ExtractParams;
use tokio::task::JoinSet;

use crate::SearchResult;
use crate::follow::TimedSearchOutcome;

/// Configuration for a batch search operation.
#[derive(Clone)]
pub struct BatchSearchConfig {
    /// If `true`, attempt to load each result URL (best-effort).
    pub follow_results: bool,
    /// Max characters when following (ignored if `follow_results` is `false`).
    pub follow_max_chars: usize,
    /// Optional domain reputation cache.
    pub reputation: Option<Arc<DomainReputation>>,
}

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
    /// Before following a URL, the domain reputation cache is consulted;
    /// blocked domains are skipped without opening CDP tabs. After a
    /// real extraction, detected quality flags are written back to the
    /// reputation cache.
    ///
    /// # Arguments
    ///
    /// * `session` — Shared CDP session (wrapped in [`Arc`] for concurrent access).
    /// * `queries` — List of search queries.
    /// * `count` — Results per query.
    /// * `config` — Batch search configuration (follow, reputation, etc.).
    pub async fn search(
        session: Arc<Session>,
        queries: &[String],
        count: usize,
        config: BatchSearchConfig,
    ) -> Result<Vec<Vec<SearchResult>>, CdpError> {
        let timeout = Duration::from_secs(30);
        let mut join_set: JoinSet<Result<Vec<SearchResult>, CdpError>> = JoinSet::new();

        for query in queries {
            let session = Arc::clone(&session);
            let query = query.clone();
            let config = config.clone();

            join_set
                .spawn(async move { search_single(session, query, count, timeout, config).await });
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

/// Per-query search logic used by [`BatchProcessor::search`].
///
/// Uses [`crate::follow::search_with_tab`] for the search phase, then
/// optionally follows result URLs (each in a fresh tab).
async fn search_single(
    session: Arc<Session>,
    query: String,
    count: usize,
    timeout: Duration,
    config: BatchSearchConfig,
) -> Result<Vec<crate::SearchResult>, CdpError> {
    // 1. Search (tab lifecycle managed by helper)
    let outcome = crate::follow::search_with_tab(&session, &query, count, timeout).await?;

    let results = match outcome {
        TimedSearchOutcome::Success(results) => results,
        TimedSearchOutcome::Error(e) => return Err(e),
        TimedSearchOutcome::Timeout => {
            return Err(CdpError::CdpCallFailed {
                method: "batch_search".into(),
                detail: format!("timeout for query: {query}"),
            });
        }
    };

    // 2. If follow_results is enabled, follow each URL in a fresh tab
    if config.follow_results {
        for result in &results {
            let host = gthings_common::extract_host(&result.url).unwrap_or_else(|| {
                tracing::warn!("batch: failed to parse host from URL: {}", result.url);
                String::new()
            });

            // Skip blocked domains without CDP interaction
            if let Some(ref rep) = config.reputation {
                if !host.is_empty() && rep.is_blocked(&host).await {
                    tracing::debug!(url = %result.url, "batch: skip blocked domain");
                    continue;
                }
            }

            let params = ExtractParams {
                offset: 0,
                max_chars: config.follow_max_chars,
            };
            // Create a fresh tab for each follow
            if let Err(e) =
                follow_with_tab(&session, &result.url, params, config.reputation.as_deref()).await
            {
                tracing::warn!(url = %result.url, error = %e, "batch: follow failed");
            }
        }
    }

    Ok(results)
}

/// Follow a single URL inside a temporary tab.
async fn follow_with_tab(
    session: &Session,
    url: &str,
    params: ExtractParams,
    reputation: Option<&DomainReputation>,
) -> Result<(), CdpError> {
    let tab = session.create_background_tab().await?;
    let result = crate::follow::follow(session, &tab, url, params, reputation).await;
    if let Err(e) = session.close_tab(tab).await {
        tracing::warn!("close_tab failed: {e}");
    }
    result.map(|_| ())
}
