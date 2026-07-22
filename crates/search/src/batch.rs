//! Batch operations — search, follow, and the two-phase harvest pipeline.
//!
//! Provides [`BatchProcessor`] which orchestrates multi-query searches,
//! multi-URL page following, and a two-phase harvest pipeline that first
//! searches all queries (dedup + rank) and then follows the top M results.

use std::time::Instant;

use common::GthingsError;
use common::config::GthingsConfig;

use crate::follow::PageFollower;
use crate::search::GoogleSearch;
use crate::types::{
    BatchSearchResult, FollowOpts, FollowResult, HarvestMeta, HarvestResult, SearchResult,
};

/// Batch processor for multi-step search pipelines.
///
/// Combines [`GoogleSearch`] and [`PageFollower`] into higher-level
/// batch operations:
///
/// - **`search`** — multi-query search with dedup and ranking.
/// - **`follow`** — multi-URL page extraction.
/// - **`harvest`** — two-phase pipeline: search then follow the best results.
pub struct BatchProcessor {
    searcher: GoogleSearch,
    follower: PageFollower,
}

impl BatchProcessor {
    /// Create a new [`BatchProcessor`].
    pub fn new(config: GthingsConfig) -> Self {
        Self {
            searcher: GoogleSearch::new(config.clone()),
            follower: PageFollower::new(config),
        }
    }

    /// Batch search: run multiple queries, deduplicate by URL, rank by
    /// snippet length descending.
    ///
    /// Delegates to the `batch-search` subprocess for Phase 1.
    ///
    /// # Errors
    ///
    /// Returns [`GthingsError`] if any subprocess call fails.
    pub async fn search(
        &self,
        queries: &[String],
        count: usize,
    ) -> Result<BatchSearchResult, GthingsError> {
        self.searcher.batch(queries, count).await
    }

    /// Batch follow: follow multiple URLs in parallel browser tabs.
    ///
    /// Delegates to the `batch-follow` subprocess for Phase 1.
    ///
    /// # Errors
    ///
    /// Returns [`GthingsError`] if any subprocess call fails.
    pub async fn follow(
        &self,
        urls: &[String],
        opts: FollowOpts,
    ) -> Result<Vec<FollowResult>, GthingsError> {
        self.follower.batch(urls, opts).await
    }

    /// Two-phase harvest pipeline.
    ///
    /// 1. **Phase 1** — Search all `queries` and collect results.
    ///    Results are deduplicated by URL and ranked by snippet length.
    ///
    /// 2. **Phase 2** — Follow the top `max_pages` results (by snippet
    ///    length) to extract full page content.
    ///
    /// Returns a [`HarvestResult`] containing both the raw search results
    /// and the followed pages, along with pipeline metadata.
    ///
    /// # Errors
    ///
    /// Returns [`GthingsError`] if any subprocess call fails. If Phase 1
    /// returns zero results, Phase 2 is skipped and an empty harvest
    /// result is returned.
    pub async fn harvest(
        &self,
        queries: &[String],
        count: usize,
        max_pages: usize,
    ) -> Result<HarvestResult, GthingsError> {
        let start = Instant::now();

        // ── Phase 1: Batch search ────────────────────────────────────────
        tracing::debug!(
            n_queries = queries.len(),
            count,
            "harvest: Phase 1 — batch search"
        );

        let batch_result = self.searcher.batch(queries, count).await?;
        let search_results = batch_result.results;
        let total_search_results = search_results.len();

        if search_results.is_empty() {
            tracing::warn!("harvest: Phase 1 returned zero results");
            return Ok(HarvestResult {
                search_results: Vec::new(),
                read_pages: Vec::new(),
                meta: HarvestMeta {
                    queries: queries.to_vec(),
                    total_search_results: 0,
                    unique_urls: 0,
                    pages_followed: 0,
                    pages_skipped: 0,
                    duration_ms: start.elapsed().as_millis() as u64,
                },
            });
        }

        // ── Rank by snippet length (descending) and take top M ───────────
        let mut ranked: Vec<&SearchResult> = search_results.iter().collect();
        ranked.sort_by(|a, b| b.snippet.len().cmp(&a.snippet.len()));
        let top: Vec<&SearchResult> = ranked.into_iter().take(max_pages).collect();

        tracing::debug!(
            total_results = total_search_results,
            top_n = top.len(),
            "harvest: Phase 2 — follow top pages"
        );

        // ── Phase 2: Batch follow ────────────────────────────────────────
        let urls_to_follow: Vec<String> = top.iter().map(|r| r.url.clone()).collect();
        let opts = FollowOpts::default();

        let read_pages = self.follower.batch(&urls_to_follow, opts).await?;

        let elapsed = start.elapsed().as_millis() as u64;
        let pages_followed = read_pages.len();
        let unique_urls = {
            let mut seen = std::collections::HashSet::new();
            for r in &search_results {
                seen.insert(r.url.clone());
            }
            seen.len()
        };
        let pages_skipped = read_pages.iter().filter(|p| p.error.is_some()).count();

        tracing::debug!(
            pages_followed,
            elapsed_ms = elapsed,
            "harvest: pipeline complete"
        );

        Ok(HarvestResult {
            search_results,
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
        // Just verify it doesn't panic
        let _ = bp;
    }
}
