//! Batch operations — concurrent multi-query search.
//!
//! All operations share a single [`Session`] (one WebSocket connection),
//! creating independent tabs per task with per-task timeout isolation.
//!
//! Queries are fault-isolated: a failure in one query is collected as a
//! per-query error entry rather than aborting the whole batch. Concurrent CDP
//! tabs are bounded by a shared [`Semaphore`], and every tab is wrapped in a
//! [`TabGuard`] so it closes on all exit paths (including cancellation).

use std::sync::Arc;
use std::time::Duration;

use gthings_cdp::{CdpError, Session, TabGuard};
use gthings_common::domain_reputation::DomainReputation;
use gthings_common::pagination::ExtractParams;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::SearchResult;
use crate::engine::router::SearchRouter;
use crate::engine::{MAX_CONCURRENT_TABS, OP_TIMEOUT};
use crate::follow::TimedSearchOutcome;
use crate::search::search_with_router_tab;

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
    /// Queries are fault-isolated: each query yields its own
    /// `Result<Vec<SearchResult>, CdpError>` entry, so a failure in one query
    /// does not abort the others. The batch only fails as a whole on a genuine
    /// [`JoinError`] (a spawned task panicked).
    ///
    /// Concurrent CDP tabs are bounded by a shared [`Semaphore`] (acquired
    /// before a tab is created, released on completion), preventing browser
    /// resource exhaustion on large batches.
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
    ) -> Result<Vec<Result<Vec<SearchResult>, CdpError>>, CdpError> {
        let timeout = OP_TIMEOUT;
        // ONE shared router for the whole batch: every concurrent query
        // dispatches through the same `SearchRouter`, so its single in-memory
        // `PacingStore` coordinates engine pacing across the entire batch
        // (pick_and_reserve + the wait loop serialize engine dispatch). The
        // routing mode is resolved from `GTHINGS_ENGINE_MODE` (a daemon-level
        // concern) inside `SearchRouter::new`.
        let router = Arc::new(SearchRouter::new(Some(Arc::clone(&session))));
        // ONE shared semaphore bounds concurrent CDP tabs across the batch.
        let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_TABS));
        let mut join_set: JoinSet<Result<Vec<SearchResult>, CdpError>> = JoinSet::new();

        for query in queries {
            let session = Arc::clone(&session);
            let router = Arc::clone(&router);
            let semaphore = Arc::clone(&semaphore);
            let query = query.clone();
            let config = config.clone();

            join_set.spawn(async move {
                search_single(session, router, semaphore, query, count, timeout, config).await
            });
        }

        collect_results(join_set, queries.len()).await
    }
}

/// Collect per-query results from the [`JoinSet`], aborting only on a genuine
/// [`JoinError`] (a spawned task panicked). Query failures are collected as
/// per-query `Err` entries rather than failing the whole batch.
async fn collect_results(
    mut join_set: JoinSet<Result<Vec<SearchResult>, CdpError>>,
    len: usize,
) -> Result<Vec<Result<Vec<SearchResult>, CdpError>>, CdpError> {
    let mut all_results = Vec::with_capacity(len);
    while let Some(task) = join_set.join_next().await {
        match task {
            Ok(result) => all_results.push(result),
            Err(join_err) => {
                return Err(crate::harvest::orchestrator::map_join_err(
                    "batch_search",
                    join_err,
                ));
            }
        }
    }
    Ok(all_results)
}

/// Per-query search logic used by [`BatchProcessor::search`].
///
/// Acquires a semaphore permit (bounding concurrent CDP tabs) before running
/// the search phase through the batch-shared router (see
/// [`crate::search::search_with_router_tab`]), then optionally follows result
/// URLs (each in a fresh tab). The permit auto-releases on drop, including
/// cancellation.
async fn search_single(
    session: Arc<Session>,
    router: Arc<SearchRouter>,
    semaphore: Arc<Semaphore>,
    query: String,
    count: usize,
    timeout: Duration,
    config: BatchSearchConfig,
) -> Result<Vec<crate::SearchResult>, CdpError> {
    // Acquire a permit BEFORE creating a tab; OwnedSemaphorePermit auto-releases
    // on drop (including cancellation), bounding concurrent CDP tabs.
    let _permit = semaphore
        .acquire_owned()
        .await
        .map_err(|e| CdpError::CdpCallFailed {
            method: "batch_search".into(),
            detail: format!("semaphore acquire failed: {e}"),
        })?;

    // 1. Search (tab lifecycle managed by helper; shared router → cross-query
    //    pacing via the single in-memory PacingStore)
    let outcome = search_with_router_tab(router.clone(), &session, &query, count, timeout).await?;

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
    // RAII guard closes the tab on ALL exit paths (success, error, cancellation).
    let _guard = TabGuard::new(session, tab.clone());
    crate::follow::follow(session, &tab, url, params, reputation)
        .await
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{EngineChoice, EngineMode, SearchEngine, SearchEngineError};
    use crate::search::search_with_router;
    use gthings_common::provenance::Provenance;
    use std::time::Instant;

    fn make_result(url: &str) -> SearchResult {
        SearchResult {
            title: "Title".into(),
            url: url.to_string(),
            snippet: "Snippet".into(),
            position: 1,
            provenance: Provenance::default(),
            domain_authority: 0.5,
            source_type: "web".into(),
            engine: SearchEngine::Brave,
            score: 0.0,
            published_date: None,
            favicon: None,
            mode: EngineMode::Hybrid,
        }
    }

    /// Batch queries must share ONE router (and thus one in-memory
    /// `PacingStore`), so a reservation made by one query is visible to the
    /// next. A router built with no browser session fails pinned Google fast
    /// (Unavailable) but still stamps Google's pacing reservation before
    /// dispatch; a second query through the SAME router must observe that
    /// reservation and wait out Google's minimum interval — proving the
    /// router is shared rather than rebuilt per query.
    #[tokio::test]
    async fn shared_router_pacing_visible_across_queries() {
        let router = Arc::new(SearchRouter::new(None));

        // First "batch query": pin Google → fails fast (no browser session),
        // but stamps Google's pacing reservation under the pick lock.
        let err = search_with_router(
            router.clone(),
            "q1",
            5,
            EngineChoice::Pin(SearchEngine::Google),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, SearchEngineError::Unavailable { .. }),
            "pinned Google without a session must fail fast"
        );

        // Second "batch query" through the same router: Google is now over
        // budget, so pin mode must politely wait out the remaining interval
        // before dispatching. The wait is the observable proof that the
        // first query's reservation lives in the shared router's PacingStore.
        let start = Instant::now();
        let err = search_with_router(
            router.clone(),
            "q2",
            5,
            EngineChoice::Pin(SearchEngine::Google),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, SearchEngineError::Unavailable { .. }),
            "pinned Google without a session still fails after the pacing wait"
        );
        assert!(
            start.elapsed() >= Duration::from_secs(6),
            "second query must wait out Google's 6s minimum interval (reservation shared)"
        );
    }

    /// A failure in one query must not abort the whole batch: each query
    /// yields its own per-query result/error entry, and the batch only fails
    /// as a whole on a genuine JoinError (task panic).
    #[tokio::test]
    async fn per_query_failure_does_not_abort_batch() {
        let mut join_set: JoinSet<Result<Vec<SearchResult>, CdpError>> = JoinSet::new();
        join_set.spawn(async { Ok(vec![make_result("https://ok.example")]) });
        join_set.spawn(async {
            Err(CdpError::CdpCallFailed {
                method: "q2".into(),
                detail: "boom".into(),
            })
        });

        let results = collect_results(join_set, 2)
            .await
            .expect("batch must not abort");

        // Both queries appear in the output, each with its own result or error.
        assert_eq!(results.len(), 2);
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
        assert_eq!(results.iter().filter(|r| r.is_err()).count(), 1);
    }
}
