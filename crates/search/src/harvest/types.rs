use std::collections::HashMap;
use std::sync::Arc;

use crate::SearchResult;
use crate::engine::SearchEngine;
use gthings_common::domain_reputation::DomainReputation;
use gthings_common::pagination::{ExtractParams, Pagination};
use gthings_common::provenance::Provenance;
use gthings_extraction::article::{QualityScore, Section};
use serde::{Deserialize, Serialize};

/// Configuration for a complete harvest pipeline.
#[derive(Clone)]
pub struct BatchHarvestRequest {
    /// One or more search queries to execute.
    pub queries: Vec<String>,
    /// Strategy for ordering results after dedup.
    pub rank_by: RankStrategy,
    /// Number of top-ranked results to follow for full content extraction.
    /// Set to 0 to skip following entirely.
    pub follow_top_n: usize,
    /// Pagination/extraction parameters for following.
    pub extract_params: ExtractParams,
    /// Optional domain reputation cache. When provided, blocked domains
    /// are skipped without CDP navigation and quality flags are written
    /// back after extraction.
    pub reputation: Option<Arc<DomainReputation>>,
    /// Search engine selection for the search phase. `None` uses the router's
    /// auto mode (priority fallback across engines); `Some(e)` pins exactly
    /// one engine with no fallback.
    pub engine: Option<SearchEngine>,
}

impl std::fmt::Debug for BatchHarvestRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BatchHarvestRequest")
            .field("queries", &self.queries)
            .field("rank_by", &self.rank_by)
            .field("follow_top_n", &self.follow_top_n)
            .field("extract_params", &self.extract_params)
            .field("reputation", &self.reputation.as_ref().map(|_| "Some(...)"))
            .field("engine", &self.engine)
            .finish()
    }
}

/// Status of body content for a harvested result — lets agents triage without re-inferring
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BodyStatus {
    Ok,
    SnippetOnly,
    ExtractFailed,
    PdfUnextracted,
    ChromeOrEmpty,
}

/// Coverage stats per query
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryCoverage {
    pub total_hits: usize,
    pub followed_ok: usize,
    pub followed_failed: usize,
}

/// Warnings for the agent consumer
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarvestWarning {
    FollowBudgetCollapsedToOneSite,
    NoBodyForQuery(String),
    AllSnippetOnly,
}

/// Run-level summary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarvestRunSummary {
    pub total_queries: usize,
    pub total_results: usize,
    pub unique_sources_followed: usize,
    pub coverage_by_query: HashMap<String, QueryCoverage>,
    pub warnings: Vec<HarvestWarning>,
}

/// Ranking strategy for ordering harvested results.
#[derive(Debug, Clone)]
pub enum RankStrategy {
    /// Keep Google's original SERP order, interleaving results from
    /// multiple queries round-robin.
    SerpOrder,
    /// Sort descending by domain authority score.
    DomainAuthority,
    /// Sort descending by snippet length.
    SnippetLength,
    /// Composite score = 0.5×authority + 0.3×norm_snippet + 0.2×diversity_bonus.
    Composite,
}

/// A single harvested result — combines search metadata with optional
/// followed content and quality assessment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarvestedResult {
    /// The original search result metadata.
    pub search_result: SearchResult,
    /// Full page content (if follow was attempted and succeeded).
    pub followed_content: Option<String>,
    /// Provenance of the content acquisition.
    pub provenance: Provenance,
    /// Pagination state of the followed content.
    pub pagination: Option<Pagination>,
    /// Quality assessment of the followed content.
    pub quality: Option<QualityScore>,
    /// Sections extracted from the followed content.
    pub sections: Vec<Section>,
    /// Canonicalized URL (always present, based on search_result.url).
    pub url_canonical: String,
    /// The query that produced this result.
    pub query: String,
    /// Status of body content for agent triage.
    pub body_status: BodyStatus,
}
