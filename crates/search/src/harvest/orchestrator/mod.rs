//! Orchestration: phase_search, phase_follow, harvest, and helpers.

mod follow;
mod junk;
mod search;

#[cfg(test)]
mod tests;

pub(crate) use junk::is_junk_url;
pub(crate) use search::map_join_err;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use gthings_cdp::{CdpError, Session};
use gthings_common::url_normalizer::registered_domain;

use super::ranking::{dedup_results, rank_results};
use super::types::*;

/// Acquire a semaphore permit, bounding the wait with `timeout`. On failure
/// (closed semaphore or timeout) the `on_fail` closure produces the value to
/// return instead of blocking indefinitely. Shared by the search and follow
/// phases so both bound their concurrency identically.
pub(crate) async fn acquire_permit<T>(
    semaphore: Arc<tokio::sync::Semaphore>,
    timeout: Duration,
    on_fail: impl FnOnce() -> T,
) -> Result<tokio::sync::OwnedSemaphorePermit, T> {
    match tokio::time::timeout(timeout, semaphore.acquire_owned()).await {
        Ok(Ok(permit)) => Ok(permit),
        Ok(Err(_)) | Err(_) => Err(on_fail()),
    }
}

/// Construct an empty [`HarvestRunSummary`] for the early-return path.
fn empty_summary(total_queries: usize) -> HarvestRunSummary {
    HarvestRunSummary {
        total_queries,
        total_results: 0,
        unique_sources_followed: 0,
        coverage_by_query: HashMap::new(),
        warnings: Vec::new(),
    }
}

/// Run the full research pipeline: search → dedup → rank → follow.
///
/// 1. **Search** — Runs all queries in parallel using [`tokio::task::JoinSet`]
///    through the multi-engine [`crate::engine::router::SearchRouter`] (auto
///    fallback or pinned engine).
/// 2. **Dedup** — Removes duplicate normalized URLs, keeping first occurrence.
/// 3. **Rank** — Orders results by the chosen [`RankStrategy`].
/// 4. **Follow** — Follows the top `follow_top_n` results in parallel using
///    [`tokio::task::JoinSet`].
///
/// Returns harvested results sorted by rank. When a follow fails, the result
/// is still included with `followed_content = None` and a quality score of 0.
///
/// Tabs are created and closed per-task. The session must be wrapped in [`Arc`]
/// so it can be shared across concurrent tasks.
pub async fn harvest(
    session: Arc<Session>,
    req: BatchHarvestRequest,
) -> Result<(Vec<HarvestedResult>, HarvestRunSummary), CdpError> {
    // Phase 1: Parallel search
    let raw = search::phase_search(Arc::clone(&session), &req).await?;

    if raw.is_empty() {
        let empty_summary = empty_summary(req.queries.len());
        return Ok((Vec::new(), empty_summary));
    }

    // Phase 2: Merge, dedup, rank (CPU-only)
    let deduped = dedup_results(raw);
    let ranked = rank_results(deduped, &req.rank_by);

    // Phase 2b: Select follow candidates with diversity
    let (selected, domains_selected) = follow::select_follow_candidates(ranked, req.follow_top_n);

    // Check if all selected candidates come from the same domain
    let mut warnings: Vec<HarvestWarning> = Vec::new();
    if domains_selected.len() <= 1 && selected.len() > 1 {
        warnings.push(HarvestWarning::FollowBudgetCollapsedToOneSite);
    }

    // Phase 3: Parallel follow
    let harvested = follow::phase_follow(
        session,
        selected,
        req.follow_top_n,
        req.extract_params,
        req.reputation,
    )
    .await;

    // Build run summary
    let mut coverage: HashMap<String, QueryCoverage> = HashMap::new();
    let mut unique_domains: HashSet<String> = HashSet::new();

    for result in &harvested {
        let entry = coverage
            .entry(result.query.clone())
            .or_insert(QueryCoverage {
                total_hits: 0,
                followed_ok: 0,
                followed_failed: 0,
            });
        entry.total_hits += 1;
        match &result.body_status {
            BodyStatus::Ok => entry.followed_ok += 1,
            _ => entry.followed_failed += 1,
        }
        if let Some(domain) = registered_domain(&result.url_canonical) {
            unique_domains.insert(domain);
        }
    }

    // Check for queries with no OK result
    for (q, c) in &coverage {
        if c.followed_ok == 0 && c.total_hits > 0 {
            warnings.push(HarvestWarning::NoBodyForQuery(q.clone()));
        }
    }

    // Check if all non-PDF results are snippet-only
    let non_pdf = harvested
        .iter()
        .filter(|r| !matches!(r.body_status, BodyStatus::PdfUnextracted))
        .count();
    let all_empty = non_pdf > 0
        && harvested
            .iter()
            .filter(|r| !matches!(r.body_status, BodyStatus::Ok))
            .count()
            == non_pdf;
    if all_empty {
        warnings.push(HarvestWarning::AllSnippetOnly);
    }

    let summary = HarvestRunSummary {
        total_queries: req.queries.len(),
        total_results: harvested.len(),
        unique_sources_followed: unique_domains.len(),
        coverage_by_query: coverage,
        warnings,
    };

    Ok((harvested, summary))
}
