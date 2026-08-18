//! Phase 1: parallel search.

use std::sync::Arc;

use gthings_cdp::{CdpError, Session};
use tokio::task::JoinSet;

use crate::SearchResult;
use crate::engine::EngineChoice;
use crate::engine::SearchOptions;
use crate::engine::router::{SearchRouter, map_engine_results};

use super::super::types::*;

/// Convert a [`tokio::task::JoinError`] into a [`CdpError`].
pub(crate) fn map_join_err(method: &str, err: tokio::task::JoinError) -> CdpError {
    CdpError::CdpCallFailed {
        method: method.into(),
        detail: format!("join error: {err}"),
    }
}

// ---------------------------------------------------------------------------
// Phase 1: Parallel search
// ---------------------------------------------------------------------------

/// Execute all search queries in parallel using a [`JoinSet`] and the shared
/// [`SearchRouter`].
///
/// The router is built once per call (with the shared session) and cloned into
/// each task. Every task runs [`SearchRouter::search_with_fallback`] with a
/// 30-second per-task timeout. Timeouts and per-query engine failures degrade
/// gracefully to empty result vectors for that query, so one failing engine
/// never kills the harvest.
pub(crate) async fn phase_search(
    session: Arc<Session>,
    req: &BatchHarvestRequest,
) -> Result<Vec<(String, SearchResult)>, CdpError> {
    // Build the router once per phase_search call and derive the engine choice.
    // The routing mode is resolved by the router from `GTHINGS_ENGINE_MODE`
    // (a daemon-level concern), never threaded from the CLI.
    let router = Arc::new(SearchRouter::new(Some(Arc::clone(&session))));
    let choice = match req.engine {
        Some(e) => EngineChoice::Pin(e),
        None => EngineChoice::Auto,
    };

    let mut search_join_set: JoinSet<Result<(String, Vec<SearchResult>), CdpError>> =
        JoinSet::new();
    let count = 10;
    let search_timeout = crate::engine::OP_TIMEOUT;

    // Cap the number of in-flight searches so queries dispatch in waves rather
    // than all at once. With in-memory pacing and Brave's 5s minimum interval,
    // only a few queries can dispatch per window; a semaphore lets the rest
    // queue and run as permits free up instead of all failing at once.
    let search_semaphore = Arc::new(tokio::sync::Semaphore::new(
        crate::engine::MAX_CONCURRENT_TABS,
    ));

    tracing::info!(
        "harvest search: spawning {} parallel queries",
        req.queries.len()
    );

    for query in &req.queries {
        let router = Arc::clone(&router);
        let query = query.clone();
        let semaphore = Arc::clone(&search_semaphore);

        search_join_set.spawn(async move {
            // Acquire a permit before dispatching; released when the task ends.
            // Bound the wait so queued tasks give up rather than waiting
            // unboundedly behind the 4-permit cap.
            let _permit = match super::acquire_permit(semaphore, search_timeout, || {
                tracing::warn!("search semaphore acquire failed for query: {query}");
                (query.clone(), Vec::new())
            })
            .await
            {
                Ok(permit) => permit,
                Err(fail) => return Ok(fail),
            };
            let started = std::time::Instant::now();
            match tokio::time::timeout(
                search_timeout,
                router.search_with_fallback(&query, count, choice, &SearchOptions::default()),
            )
            .await
            {
                Ok(Ok(engine_results)) => {
                    let duration_ms = started.elapsed().as_millis() as u64;
                    let results =
                        map_engine_results(engine_results, &query, duration_ms, router.mode());
                    Ok((query, results))
                }
                Ok(Err(e)) => {
                    tracing::warn!("phase_search: query {query:?} failed: {e}");
                    Ok((query, Vec::new()))
                }
                Err(_) => {
                    tracing::warn!("harvest search timed out for query: {query}");
                    Ok((query, Vec::new()))
                }
            }
        });
    }

    let mut raw: Vec<(String, SearchResult)> = Vec::new();
    while let Some(task_result) = search_join_set.join_next().await {
        match task_result {
            Ok(Ok((query, results))) => {
                for r in results {
                    raw.push((query.clone(), r));
                }
            }
            Ok(Err(e)) => return Err(e),
            Err(join_err) => return Err(map_join_err("harvest_search", join_err)),
        }
    }

    Ok(raw)
}
