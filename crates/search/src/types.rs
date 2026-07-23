//! Shared data types for search operations.
//!
//! These types are used across the search, follow, and batch modules
//! to represent search results, page content, and pipeline metadata.

/// A single search result returned by a search engine.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SearchResult {
    /// Page title.
    pub title: String,
    /// Result URL.
    pub url: String,
    /// Text snippet shown in search results.
    pub snippet: String,
    /// Originating query (set during batch operations for dedup tracking).
    #[serde(default)]
    pub query: Option<String>,
}

/// Metadata for a single search operation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SearchMeta {
    /// Number of unique results returned.
    pub total: usize,
    /// The search query (or comma-joined for batch).
    pub query: String,
    /// Wall-clock duration of the operation in milliseconds.
    pub duration_ms: u64,
}

/// Result of following a single URL and extracting its content.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FollowResult {
    /// Whether the follow operation succeeded (content was extracted).
    pub success: bool,
    /// The URL that was followed.
    pub url: String,
    /// Extracted text content, if available.
    pub content: Option<String>,
    /// Total text length on the full page (before any offset/max truncation).
    pub total_length: usize,
    /// Character offset into the full page where extraction started.
    pub offset: usize,
    /// Whether the content was truncated (page text exceeded offset + max_length).
    pub truncated: bool,
    /// Detected content sections (heading + content pairs).
    pub sections: Vec<extraction::html::Section>,
    /// Error message if the operation failed.
    pub error: Option<String>,
    /// Content quality assessment, if computed.
    pub quality: Option<extraction::quality::QualityResult>,
}

/// Result of a batch search operation (multiple queries, deduplicated).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BatchSearchResult {
    /// Deduplicated, ranked search results.
    pub results: Vec<SearchResult>,
    /// Aggregate metadata.
    pub meta: SearchMeta,
}

/// Result of a two-phase harvest pipeline (search + follow).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HarvestResult {
    /// All search results from Phase 1.
    pub search_results: Vec<SearchResult>,
    /// Followed (read) pages from Phase 2.
    pub read_pages: Vec<FollowResult>,
    /// Aggregate pipeline metadata.
    pub meta: HarvestMeta,
}

/// Metadata for a harvest pipeline run.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HarvestMeta {
    /// All queries that were searched.
    pub queries: Vec<String>,
    /// Total number of search results across all queries.
    pub total_search_results: usize,
    /// Number of unique URLs found across all queries.
    pub unique_urls: usize,
    /// Number of pages that were followed in Phase 2.
    pub pages_followed: usize,
    /// Number of pages skipped (error) in Phase 2.
    pub pages_skipped: usize,
    /// Wall-clock duration of the entire pipeline in milliseconds.
    pub duration_ms: u64,
}

/// Options for the [`PageFollower`](crate::PageFollower) operation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct FollowOpts {
    /// CSS selector to identify the main content element.
    pub selector: String,
    /// Character offset into the full page text to start extraction.
    pub offset: usize,
    /// Maximum characters to extract.
    pub max_length: usize,
    /// Per-page navigation timeout in milliseconds.
    pub timeout_ms: u64,
    /// Whether to retry with fallback selector if content quality is low.
    pub retry_on_low_quality: bool,
    /// Number of pages to fetch concurrently.
    pub concurrency: usize,
}

impl Default for FollowOpts {
    fn default() -> Self {
        Self {
            selector: "article,main,[role=main]".into(),
            offset: 0,
            max_length: 15000,
            timeout_ms: 30000,
            retry_on_low_quality: true,
            concurrency: 1,
        }
    }
}

