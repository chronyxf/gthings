//! Full research pipeline: search → dedup → rank → follow.
//!
//! Orchestrates multiple search queries, deduplicates results by normalized URL,
//! applies a configurable ranking strategy, and follows the top-N results for
//! full content extraction with quality scoring.
//!
//! Phase 1 (search) and Phase 4 (follow) use parallel tab execution via
//! [`JoinSet`] with per-task 30-second timeouts. Tabs are always closed
//! on every exit path (success, timeout, error).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use gthings_cdp::{CdpError, Session};
use gthings_common::domain_reputation::DomainReputation;
use gthings_common::pagination::{ExtractParams, Pagination};
use gthings_common::provenance::{ExtractionMethod, Provenance};
use gthings_common::url_normalizer::{
    canonicalize_url, dedup_key, is_arxiv_url, is_pdf_url, registered_domain,
};
use gthings_extraction::article::{QualityScore, Section};
use gthings_extraction::quality::entropy::shannon_entropy;
use serde::{Deserialize, Serialize};
use tokio::task::JoinSet;
use url::Url;

use crate::SearchResult;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Configuration for a complete harvest pipeline.
#[derive(Clone)]
pub struct BatchHarvestRequest {
    /// One or more search queries to execute.
    pub queries: Vec<String>,
    /// Strategy for removing duplicate results.
    pub dedup: DedupStrategy,
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
}

impl std::fmt::Debug for BatchHarvestRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BatchHarvestRequest")
            .field("queries", &self.queries)
            .field("dedup", &self.dedup)
            .field("rank_by", &self.rank_by)
            .field("follow_top_n", &self.follow_top_n)
            .field("extract_params", &self.extract_params)
            .field("reputation", &self.reputation.as_ref().map(|_| "Some(...)"))
            .finish()
    }
}

/// URL deduplication strategy.
#[derive(Debug, Clone)]
pub enum DedupStrategy {
    /// Normalize URLs (strip tracking params, lowercase scheme+host,
    /// strip trailing slash) and keep the first occurrence.
    UrlOnly,
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

// ---------------------------------------------------------------------------
// Phase 1: Parallel search
// ---------------------------------------------------------------------------

/// Execute all search queries in parallel using a [`JoinSet`], one tab per query.
///
/// Each task creates a tab, runs [`crate::search::search`] with a 30-second
/// per-task timeout, and always closes the tab on every exit path (success,
/// timeout, error). Timeouts produce empty result vectors for that query.
async fn phase_search(
    session: Arc<Session>,
    queries: &[String],
) -> Result<Vec<(String, SearchResult)>, CdpError> {
    let mut search_join_set: JoinSet<Result<(String, Vec<SearchResult>), CdpError>> =
        JoinSet::new();
    let count = 10;
    let search_timeout = Duration::from_secs(30);

    tracing::info!("harvest search: spawning {} parallel tabs", queries.len());

    for query in queries {
        let session = Arc::clone(&session);
        let query = query.clone();

        search_join_set.spawn(async move {
            // 1. Create tab outside timeout — guarantees we can close it on all paths
            let tab = match session.create_tab("about:blank").await {
                Ok(t) => t,
                Err(e) => return Err(e),
            };

            // 2. Search with per-task timeout
            let result = tokio::time::timeout(
                search_timeout,
                crate::search::search(&session, &tab, &query, count),
            )
            .await;

            // 3. ALWAYS close tab — runs on success, timeout, and error
            if let Err(e) = session.close_tab(tab).await {
                tracing::warn!("failed to close tab: {e}");
            }

            // 4. Convert timeout to empty results for this query (no crash)
            match result {
                Ok(Ok(results)) => Ok((query, results)),
                Ok(Err(e)) => Err(e),
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
            Err(join_err) => {
                return Err(CdpError::CdpCallFailed {
                    method: "harvest_search".into(),
                    detail: format!("join error: {join_err}"),
                });
            }
        }
    }

    Ok(raw)
}

// ---------------------------------------------------------------------------
// Follow selection with diversity
// ---------------------------------------------------------------------------

/// Junk URL patterns to exclude from follow.
fn is_junk_url(url: &str) -> bool {
    let lower = url.to_lowercase();
    // google.com/accounts/*, google.com/support/*, etc.
    if lower.starts_with("https://accounts.google.com/")
        || lower.starts_with("https://support.google.com/")
        || lower.starts_with("https://policies.google.com/")
    {
        return true;
    }
    // Generic junk patterns
    if lower.contains("/track?")
        || lower.contains("doubleclick.net")
        || lower.contains("googlesyndication.com")
    {
        return true;
    }
    false
}

/// Select up to `max` URLs with diversity constraints:
/// 1. Dedup on dedup_key() from url_normalizer
/// 2. Per-query minimum — try include ≥1 hit from each query
/// 3. Per-canonical-host cap — max 2 follows per registered domain
/// 4. Remove known junk (google.com/accounts/*, google.com/support/*, etc.)
fn select_follow_candidates(
    ranked: Vec<(String, SearchResult)>,
    max: usize,
) -> Vec<(String, SearchResult)> {
    if max == 0 || ranked.is_empty() {
        return Vec::new();
    }

    // Stage 1: dedup on dedup_key() and remove known junk
    let mut seen_key = HashSet::new();
    let deduped: Vec<(String, SearchResult)> = ranked
        .into_iter()
        .filter(|(_, r)| !is_junk_url(&r.url) && seen_key.insert(dedup_key(&r.url)))
        .collect();

    if max >= deduped.len() {
        return deduped;
    }

    // Stage 2: group by query for diversity picking
    let mut by_query: HashMap<String, Vec<(String, SearchResult)>> = HashMap::new();
    for item in deduped {
        by_query.entry(item.0.clone()).or_default().push(item);
    }
    let query_keys: Vec<String> = by_query.keys().cloned().collect();

    let mut selected: Vec<(String, SearchResult)> = Vec::with_capacity(max);
    let mut domain_count: HashMap<String, usize> = HashMap::new();

    // First pass: round-robin — pick one per query respecting domain cap
    let mut any_picked = true;
    while any_picked && selected.len() < max {
        any_picked = false;
        for qk in &query_keys {
            if selected.len() >= max {
                break;
            }
            if let Some(items) = by_query.get_mut(qk) {
                if let Some(pos) = items.iter().position(|(_, r)| {
                    let domain = registered_domain(&r.url).unwrap_or_default();
                    *domain_count.get(&domain).unwrap_or(&0) < 2
                }) {
                    let item = items.remove(pos);
                    let domain = registered_domain(&item.1.url).unwrap_or_default();
                    *domain_count.entry(domain).or_insert(0) += 1;
                    selected.push(item);
                    any_picked = true;
                }
            }
        }
    }

    // Second pass: fill remaining slots from any query, respecting domain cap
    for qk in &query_keys {
        if selected.len() >= max {
            break;
        }
        if let Some(items) = by_query.get_mut(qk) {
            while let Some(pos) = items.iter().position(|(_, r)| {
                if selected.len() >= max {
                    return false;
                }
                let domain = registered_domain(&r.url).unwrap_or_default();
                *domain_count.get(&domain).unwrap_or(&0) < 2
            }) {
                let item = items.remove(pos);
                let domain = registered_domain(&item.1.url).unwrap_or_default();
                *domain_count.entry(domain).or_insert(0) += 1;
                selected.push(item);
            }
        }
    }

    selected
}

// ---------------------------------------------------------------------------
// Phase 3: Parallel follow
// ---------------------------------------------------------------------------

/// Follow the top `follow_top_n` URLs in parallel using a [`JoinSet`], one tab
/// per URL.
///
/// Results that fall within `follow_top_n` get a follow task spawned; results
/// beyond that count are included as placeholders without following. Each
/// follow task creates a tab, runs [`crate::follow::follow`] with a 30-second
/// timeout, and always closes the tab on every path.
///
/// When a follow fails (error or timeout), the result is still returned with
/// `followed_content = None` and a [`QualityScore`] of 0.
async fn phase_follow(
    session: Arc<Session>,
    ranked: Vec<(String, SearchResult)>,
    follow_top_n: usize,
    params: ExtractParams,
    reputation: Option<Arc<DomainReputation>>,
) -> Vec<HarvestedResult> {
    let mut harvested: Vec<HarvestedResult> = Vec::with_capacity(ranked.len());
    let follow_count = follow_top_n.min(ranked.len());
    let mut follow_join_set: JoinSet<(usize, HarvestedResult)> = JoinSet::new();

    if follow_count > 0 {
        tracing::info!("harvest follow: spawning {follow_count} parallel tabs");
    }

    let rep_outer = reputation;

    for (idx, (query, search_result)) in ranked.into_iter().enumerate() {
        let parsed_url = url::Url::parse(&search_result.url);
        let url_str = parsed_url
            .as_ref()
            .map(|u| u.as_str())
            .unwrap_or(&search_result.url);
        let url_canonical = canonicalize_url(url_str);
        let q = query.clone();

        if idx < follow_count {
            // Check if URL is PDF or arXiv — skip CDP follow, mark as PdfUnextracted
            if is_pdf_url(url_str) || is_arxiv_url(url_str) {
                harvested.push(HarvestedResult {
                    provenance: search_result.provenance.clone(),
                    search_result,
                    followed_content: None,
                    pagination: None,
                    quality: None,
                    sections: Vec::new(),
                    url_canonical,
                    query,
                    body_status: BodyStatus::PdfUnextracted,
                });
                continue;
            }

            // Push placeholder; will be replaced when follow completes
            let placeholder = HarvestedResult {
                provenance: search_result.provenance.clone(),
                search_result: search_result.clone(),
                followed_content: None,
                pagination: None,
                quality: None,
                sections: Vec::new(),
                url_canonical: url_canonical.clone(),
                query: q.clone(),
                body_status: BodyStatus::SnippetOnly,
            };
            harvested.push(placeholder);

            // Spawn follow task
            let session = Arc::clone(&session);
            let url = search_result.url.clone();
            let params = params.clone();
            let rep = rep_outer.clone();
            let q_task = q;

            follow_join_set.spawn(async move {
                let tab = match session.create_tab("about:blank").await {
                    Ok(t) => t,
                    Err(e) => {
                        return (
                            idx,
                            HarvestedResult {
                                search_result,
                                followed_content: None,
                                provenance: Provenance {
                                    source_url: url,
                                    method: ExtractionMethod::Follow,
                                    agent: gthings_common::GTHINGS_AGENT.into(),
                                    accessed_at: Utc::now(),
                                    duration_ms: 0,
                                    derived_from: None,
                                },
                                pagination: None,
                                quality: Some(QualityScore {
                                    score: 0.0,
                                    is_ok: false,
                                    reasons: vec![format!("create_tab failed: {e}")],
                                    entropy_bits_per_char: 0.0,
                                }),
                                sections: Vec::new(),
                                url_canonical,
                                query: q_task,
                                body_status: BodyStatus::ExtractFailed,
                            },
                        );
                    }
                };

                let result = tokio::time::timeout(
                    Duration::from_secs(30),
                    crate::follow::follow(&session, &tab, &url, params, rep.as_deref()),
                )
                .await;

                if let Err(e) = session.close_tab(tab).await {
                    tracing::warn!("failed to close tab: {e}");
                }

                let harvest_result = match result {
                    Ok(Ok(fr)) => {
                        let quality = compute_quality(&fr.content);
                        let sections = extract_sections(&fr.content);
                        let body_status = if fr.content.is_empty() && fr.error.is_empty() {
                            BodyStatus::ChromeOrEmpty
                        } else if !fr.error.is_empty() {
                            BodyStatus::ExtractFailed
                        } else if quality.score < 0.3
                            && quality.reasons.iter().any(|r| {
                                r.contains("too_short") || r.contains("nav") || r.contains("chrome")
                            })
                        {
                            BodyStatus::ChromeOrEmpty
                        } else if !quality.is_ok
                            && quality.reasons.iter().any(|r| {
                                r.contains("paywall")
                                    || r.contains("bot_blocked")
                                    || r.contains("captcha")
                            })
                        {
                            BodyStatus::ExtractFailed
                        } else {
                            BodyStatus::Ok
                        };
                        HarvestedResult {
                            search_result,
                            followed_content: Some(fr.content),
                            provenance: fr.provenance,
                            pagination: fr.pagination,
                            quality: Some(quality),
                            sections,
                            url_canonical,
                            query: q_task,
                            body_status,
                        }
                    }
                    Ok(Err(e)) => HarvestedResult {
                        search_result,
                        followed_content: None,
                        provenance: Provenance {
                            source_url: url,
                            method: ExtractionMethod::Follow,
                            agent: gthings_common::GTHINGS_AGENT.into(),
                            accessed_at: Utc::now(),
                            duration_ms: 0,
                            derived_from: None,
                        },
                        pagination: None,
                        quality: Some(QualityScore {
                            score: 0.0,
                            is_ok: false,
                            reasons: vec![format!("follow failed: {e}")],
                            entropy_bits_per_char: 0.0,
                        }),
                        sections: Vec::new(),
                        url_canonical,
                        query: q_task,
                        body_status: BodyStatus::ExtractFailed,
                    },
                    Err(_) => HarvestedResult {
                        search_result,
                        followed_content: None,
                        provenance: Provenance {
                            source_url: url,
                            method: ExtractionMethod::Follow,
                            agent: gthings_common::GTHINGS_AGENT.into(),
                            accessed_at: Utc::now(),
                            duration_ms: 0,
                            derived_from: None,
                        },
                        pagination: None,
                        quality: Some(QualityScore {
                            score: 0.0,
                            is_ok: false,
                            reasons: vec!["follow timed out".into()],
                            entropy_bits_per_char: 0.0,
                        }),
                        sections: Vec::new(),
                        url_canonical,
                        query: q_task,
                        body_status: BodyStatus::ExtractFailed,
                    },
                };

                (idx, harvest_result)
            });
        } else {
            // Beyond follow_top_n — include without following
            harvested.push(HarvestedResult {
                provenance: search_result.provenance.clone(),
                search_result,
                followed_content: None,
                pagination: None,
                quality: None,
                sections: Vec::new(),
                url_canonical,
                query,
                body_status: BodyStatus::SnippetOnly,
            });
        }
    }

    // Collect followed results in any order and place at correct index
    while let Some(task_result) = follow_join_set.join_next().await {
        match task_result {
            Ok((idx, result)) => {
                harvested[idx] = result;
            }
            Err(join_err) => {
                tracing::warn!("harvest follow join error: {join_err}");
                // The placeholder remains (followed_content = None, quality = None)
            }
        }
    }

    harvested
}

/// Run the full research pipeline: search → dedup → rank → follow.
///
/// 1. **Search** — Runs all queries in parallel using [`JoinSet`], one tab per query.
/// 2. **Dedup** — Removes duplicate normalized URLs, keeping first occurrence.
/// 3. **Rank** — Orders results by the chosen [`RankStrategy`].
/// 4. **Follow** — Follows the top `follow_top_n` results in parallel using [`JoinSet`].
///
/// Returns harvested results sorted by rank. When a follow fails, the result
/// is still included with `followed_content = None` and a quality score of 0.
///
/// Tabs are created and closed per-task. The session must be wrapped in [`Arc`]
/// so it can be shared across concurrent tasks.
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

pub async fn harvest(
    session: Arc<Session>,
    req: BatchHarvestRequest,
) -> Result<(Vec<HarvestedResult>, HarvestRunSummary), CdpError> {
    // Phase 1: Parallel search
    let raw = phase_search(Arc::clone(&session), &req.queries).await?;

    if raw.is_empty() {
        let empty_summary = empty_summary(req.queries.len());
        return Ok((Vec::new(), empty_summary));
    }

    // Phase 2: Merge, dedup, rank (CPU-only)
    let deduped = dedup_results(raw, &req.dedup);
    let ranked = rank_results(deduped, &req.rank_by);

    // Phase 2b: Select follow candidates with diversity
    let selected = select_follow_candidates(ranked, req.follow_top_n);

    // Check if all selected candidates come from the same domain
    let mut domains_selected: HashSet<String> = HashSet::new();
    for (_, r) in &selected {
        if let Some(d) = registered_domain(&r.url) {
            domains_selected.insert(d);
        }
    }
    let mut warnings: Vec<HarvestWarning> = Vec::new();
    if domains_selected.len() <= 1 && selected.len() > 1 {
        warnings.push(HarvestWarning::FollowBudgetCollapsedToOneSite);
    }

    // Phase 3: Parallel follow
    let harvested = phase_follow(
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

// ---------------------------------------------------------------------------
// URL normalization
// ---------------------------------------------------------------------------

/// Normalize a URL for deduplication.
///
/// Rules:
/// - Lowercase the scheme and host.
/// - Strip tracking query parameters (`utm_*`, `fbclid`, `gclid`).
/// - Strip trailing slash from the path.
fn normalize_url(raw: &str) -> String {
    let mut u = match Url::parse(raw) {
        Ok(u) => u,
        Err(_) => return raw.to_string(),
    };

    // Lowercase scheme
    let scheme = u.scheme().to_lowercase();
    let _ = u.set_scheme(&scheme);

    // Lowercase host
    if let Some(host) = u.host_str() {
        let _ = u.set_host(Some(&host.to_lowercase()));
    }

    // Strip tracking query params
    let tracking: HashSet<&str> = [
        "utm_source",
        "utm_medium",
        "utm_campaign",
        "utm_term",
        "utm_content",
        "fbclid",
        "gclid",
    ]
    .into_iter()
    .collect();

    let pairs: Vec<(String, String)> = u
        .query_pairs()
        .filter(|(k, _)| !tracking.contains(k.as_ref()))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();

    let query = if pairs.is_empty() {
        None
    } else {
        let mut qs = String::new();
        for (i, (k, v)) in pairs.iter().enumerate() {
            if i > 0 {
                qs.push('&');
            }
            qs.push_str(&url::form_urlencoded::byte_serialize(k.as_bytes()).collect::<String>());
            qs.push('=');
            qs.push_str(&url::form_urlencoded::byte_serialize(v.as_bytes()).collect::<String>());
        }
        Some(qs)
    };
    u.set_query(query.as_deref());

    // Lowercase path
    let path = u.path().to_lowercase();

    // Strip trailing slash from path
    let path = path.trim_end_matches('/').to_string();
    u.set_path(&path);

    u.as_str().trim_end_matches('/').to_string()
}

// ---------------------------------------------------------------------------
// Dedup
// ---------------------------------------------------------------------------

/// Deduplicate search results, keeping the first occurrence of each
/// normalized URL.
fn dedup_results(
    results: Vec<(String, SearchResult)>,
    strategy: &DedupStrategy,
) -> Vec<(String, SearchResult)> {
    match strategy {
        DedupStrategy::UrlOnly => {
            let mut seen = HashSet::new();
            let mut deduped = Vec::new();
            for (query, r) in results {
                let key = dedup_key(&r.url);
                if seen.insert(key) {
                    deduped.push((query, r));
                }
            }
            deduped
        }
    }
}

// ---------------------------------------------------------------------------
// Ranking
// ---------------------------------------------------------------------------

/// Rank deduplicated results according to the chosen strategy.
fn rank_results(
    results: Vec<(String, SearchResult)>,
    strategy: &RankStrategy,
) -> Vec<(String, SearchResult)> {
    match strategy {
        RankStrategy::SerpOrder => interleave_by_query(results),
        RankStrategy::DomainAuthority => {
            let mut r = results;
            r.sort_by(|a, b| {
                b.1.domain_authority
                    .partial_cmp(&a.1.domain_authority)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            r
        }
        RankStrategy::SnippetLength => {
            let mut r = results;
            r.sort_by_key(|b| std::cmp::Reverse(b.1.snippet.len()));
            r
        }
        RankStrategy::Composite => {
            let max_snippet_len = results
                .iter()
                .map(|(_, r)| r.snippet.len())
                .max()
                .unwrap_or(1)
                .max(1);

            // Count how many queries returned each normalized URL
            let mut url_query_count: HashMap<String, usize> = HashMap::new();
            for (_, r) in &results {
                let norm = normalize_url(&r.url);
                *url_query_count.entry(norm).or_insert(0) += 1;
            }

            let mut scored: Vec<(f64, usize, String, SearchResult)> = results
                .into_iter()
                .enumerate()
                .map(|(idx, (query, r))| {
                    let norm = normalize_url(&r.url);
                    let query_count = *url_query_count.get(&norm).unwrap_or(&1);
                    let diversity_bonus = 1.0 / query_count as f64;
                    let norm_snippet = r.snippet.len() as f64 / max_snippet_len as f64;
                    let score = 0.5 * r.domain_authority as f64
                        + 0.3 * norm_snippet
                        + 0.2 * diversity_bonus;
                    // Use idx as tiebreaker for stable sorting
                    (score, idx, query, r)
                })
                .collect();

            // Sort descending by score, then by original index for stability
            scored.sort_by(|a, b| {
                b.0.partial_cmp(&a.0)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.1.cmp(&b.1))
            });

            scored
                .into_iter()
                .map(|(_, _, query, r)| (query, r))
                .collect()
        }
    }
}

/// Interleave results by query (round-robin).
///
/// Example with 3 queries: Q1R1, Q2R1, Q3R1, Q1R2, Q2R2, Q3R2, ...
fn interleave_by_query(results: Vec<(String, SearchResult)>) -> Vec<(String, SearchResult)> {
    // Group results by query, preserving insertion order via BTreeMap
    let mut by_query: BTreeMap<String, Vec<(String, SearchResult)>> = BTreeMap::new();
    for (query, r) in results {
        by_query.entry(query.clone()).or_default().push((query, r));
    }

    let mut iters: Vec<_> = by_query.into_values().map(|v| v.into_iter()).collect();

    let mut out = Vec::new();
    loop {
        let mut any = false;
        for iter in &mut iters {
            if let Some(item) = iter.next() {
                out.push(item);
                any = true;
            }
        }
        if !any {
            break;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Quality & section helpers
// ---------------------------------------------------------------------------

/// Compute a [`QualityScore`] from extracted text content.
///
/// Starts at 1.0 and subtracts for each detected issue (bot wall, paywall,
/// captcha, empty shell, too short, too few words).
fn compute_quality(content: &str) -> QualityScore {
    let mut reasons = Vec::new();
    let mut score = 1.0_f64;

    if content.is_empty() {
        return QualityScore {
            score: 0.0,
            is_ok: false,
            reasons: vec!["empty_content".into()],
            entropy_bits_per_char: 0.0,
        };
    }

    if gthings_extraction::ContentQuality::detect_bot(content) {
        reasons.push("bot_blocked".into());
        score -= 0.6;
    }
    if gthings_extraction::ContentQuality::detect_paywall(content) {
        reasons.push("paywall".into());
        score -= 0.6;
    }
    if gthings_extraction::ContentQuality::detect_captcha(content) {
        reasons.push("captcha".into());
        score -= 0.3;
    }
    if gthings_extraction::ContentQuality::detect_empty_shell(content) {
        reasons.push("empty_shell".into());
        score -= 0.2;
    }
    if content.len() < 80 {
        reasons.push("too_short".into());
        score -= 0.2;
    }
    if content.split_whitespace().count() < 15 {
        reasons.push("too_few_words".into());
        score -= 0.1;
    }

    let entropy = shannon_entropy(content);
    if entropy < 2.0 {
        reasons.push("low_entropy".into());
        score -= 0.2;
    }

    score = score.clamp(0.0, 1.0);
    let is_ok = score >= 0.5;

    // Ensure reasons is non-empty when score < 0.8 and is_ok = false
    let reasons = if reasons.is_empty() && score < 0.8 && !is_ok {
        vec!["low_quality".into()]
    } else {
        reasons
    };

    QualityScore {
        score,
        is_ok,
        reasons,
        entropy_bits_per_char: entropy,
    }
}

/// Extract section-like structure from plain text content.
///
/// Uses double-newline block splitting: if a block's first line is a short
/// line that doesn't end with sentence punctuation, it's treated as a heading.
///
/// Supports two formats:
/// - **Format A** — Heading and content in the same `\n\n` block, separated
///   by a single newline: `"Heading\nContent line 1\nContent line 2"`.
/// - **Format B** — Heading and content in separate `\n\n` blocks:
///   `"Heading\n\nContent paragraph"`.
fn extract_sections(content: &str) -> Vec<Section> {
    if content.len() < 50 {
        return Vec::new();
    }

    let mut sections = Vec::new();
    let blocks: Vec<&str> = content.split("\n\n").collect();
    let mut offset = 0;
    let mut i = 0;

    // Returns `true` if `line` looks like a section heading.
    let is_heading = |s: &str| -> bool {
        let t = s.trim();
        !t.is_empty()
            && t.len() < 100
            && !t.ends_with('.')
            && !t.ends_with('!')
            && !t.ends_with('?')
            && t.chars().filter(|&c| c == ' ').count() < 12
    };

    while i < blocks.len() {
        let raw = blocks[i];
        let block = raw.trim();
        let block_start = offset;
        offset += raw.len() + 2;

        if block.is_empty() {
            i += 1;
            continue;
        }

        let lines: Vec<&str> = block.lines().collect();

        // Format A: multi-line block with heading as first line
        if lines.len() >= 2 && is_heading(lines[0]) {
            sections.push(Section {
                heading: lines[0].trim().to_string(),
                depth: 2,
                offset: block_start,
                length: raw.len(),
                content: lines[1..].join("\n"),
                subsections: Vec::new(),
            });
            i += 1;
            continue;
        }

        // Format B: single-line heading followed by content in next block
        if lines.len() == 1 && is_heading(block) && i + 1 < blocks.len() {
            let next_raw = blocks[i + 1];
            let next_block = next_raw.trim();
            if !next_block.is_empty() {
                let next_lines: Vec<&str> = next_block.lines().collect();
                let next_is_heading =
                    next_lines.len() == 1 && next_block.len() < 100 && is_heading(next_block);

                if !next_is_heading {
                    sections.push(Section {
                        heading: block.to_string(),
                        depth: 2,
                        offset: block_start,
                        length: raw.len() + 2 + next_raw.len(),
                        content: next_block.to_string(),
                        subsections: Vec::new(),
                    });
                    // Skip the content block too
                    offset += next_raw.len() + 2;
                    i += 2;
                    continue;
                }
            }
        }

        i += 1;
    }

    sections
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use gthings_common::provenance::Provenance;

    fn make_result(url: &str, position: usize, snippet: &str, authority: f32) -> SearchResult {
        SearchResult {
            title: format!("Title {position}"),
            url: url.to_string(),
            snippet: snippet.to_string(),
            position,
            provenance: Provenance {
                source_url: "https://www.google.com/search?q=test".into(),
                method: ExtractionMethod::Search,
                agent: gthings_common::GTHINGS_AGENT.into(),
                accessed_at: Utc::now(),
                duration_ms: 100,
                derived_from: None,
            },
            domain_authority: authority,
        }
    }

    // ── Dedup tests ──────────────────────────────────────────────────────

    #[test]
    fn test_dedup_url_only_strips_tracking_params() {
        let results = vec![
            (
                "q1".into(),
                make_result(
                    "https://example.com/page?utm_source=twitter&a=1",
                    1,
                    "s1",
                    0.5,
                ),
            ),
            (
                "q1".into(),
                make_result("https://example.com/page?fbclid=abc&a=1", 2, "s2", 0.5),
            ),
        ];
        let deduped = dedup_results(results, &DedupStrategy::UrlOnly);
        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0].1.position, 1); // first occurrence kept
    }

    #[test]
    fn test_dedup_url_only_strips_trailing_slash() {
        let results = vec![
            (
                "q1".into(),
                make_result("https://Example.COM/Page/", 1, "s1", 0.5),
            ),
            (
                "q1".into(),
                make_result("https://example.com/page", 2, "s2", 0.5),
            ),
        ];
        let deduped = dedup_results(results, &DedupStrategy::UrlOnly);
        assert_eq!(deduped.len(), 1);
    }

    #[test]
    fn test_dedup_url_normalize_lowercases_host_and_path() {
        let results = vec![
            (
                "q1".into(),
                make_result("HTTP://EXAMPLE.COM/Path", 1, "s1", 0.5),
            ),
            (
                "q1".into(),
                make_result("http://example.com/path", 2, "s2", 0.5),
            ),
        ];
        let deduped = dedup_results(results, &DedupStrategy::UrlOnly);
        assert_eq!(deduped.len(), 1, "dedup_key lowercases host and path");
    }

    #[test]
    fn test_dedup_url_normalize_strips_utm() {
        let results = vec![
            (
                "q1".into(),
                make_result(
                    "https://example.com/page?utm_campaign=x&keep=1",
                    1,
                    "s1",
                    0.5,
                ),
            ),
            (
                "q1".into(),
                make_result("https://example.com/page?keep=1", 2, "s2", 0.5),
            ),
        ];
        let deduped = dedup_results(results, &DedupStrategy::UrlOnly);
        assert_eq!(deduped.len(), 1, "dedup strips utm tracking params");
        // Verify the kept result has keep=1
        assert!(deduped[0].1.url.contains("keep=1"));
    }

    // ── Composite ranking deterministic test ─────────────────────────────

    #[test]
    fn test_composite_ranking_deterministic() {
        let results = vec![
            ("q1".into(), make_result("https://a.com", 1, "short", 0.9)),
            (
                "q1".into(),
                make_result("https://b.com", 2, "a bit longer snippet here", 0.5),
            ),
            (
                "q1".into(),
                make_result("https://c.com", 3, "medium length snippet", 0.7),
            ),
        ];
        let ranked1 = rank_results(results.clone(), &RankStrategy::Composite);
        let ranked2 = rank_results(results, &RankStrategy::Composite);
        let urls1: Vec<&str> = ranked1.iter().map(|(_, r)| r.url.as_str()).collect();
        let urls2: Vec<&str> = ranked2.iter().map(|(_, r)| r.url.as_str()).collect();
        assert_eq!(urls1, urls2);
    }

    // ── Quality score tests ──────────────────────────────────────────────

    #[test]
    fn test_compute_quality_empty() {
        let q = compute_quality("");
        assert!(!q.is_ok);
        assert_eq!(q.score, 0.0);
    }

    #[test]
    fn test_compute_quality_good_content() {
        let text = "This is a sufficiently long piece of content with many words \
                     and sentences that should pass all quality checks without \
                     triggering any of the detection heuristics for bots, paywalls, \
                     or empty shells. It has plenty of text to be considered high \
                     quality content for our research purposes. We need at least \
                     80 characters and 15 words.";
        let q = compute_quality(text);
        assert!(q.is_ok);
        assert!(q.score >= 0.5);
    }

    #[test]
    fn test_compute_quality_detects_bot() {
        let text = "Checking your browser before accessing the site. Please wait while we verify you are human.";
        let q = compute_quality(text);
        assert!(!q.is_ok);
        assert!(q.reasons.iter().any(|r| r.contains("bot_blocked")));
    }

    #[test]
    fn test_compute_quality_detects_paywall() {
        let text = "Subscribe now to continue reading this article. You have reached your free article limit.";
        let q = compute_quality(text);
        assert!(!q.is_ok);
        assert!(q.reasons.iter().any(|r| r.contains("paywall")));
    }

    #[test]
    fn test_compute_quality_detects_captcha() {
        let text = "Please complete the recaptcha widget to continue.";
        let q = compute_quality(text);
        assert!(!q.is_ok);
        assert!(q.reasons.iter().any(|r| r.contains("captcha")));
    }

    // ── Section extraction tests ─────────────────────────────────────────

    #[test]
    fn test_extract_sections_empty() {
        let sections = extract_sections("");
        assert!(sections.is_empty());
    }

    #[test]
    fn test_extract_sections_finds_headings() {
        let text = "Introduction\n\nHere is some introductory content.\n\n\
                     Background\n\nThis section provides background information.\n\n\
                     Conclusion\n\nThe final section wraps up.";
        let sections = extract_sections(text);
        assert!(!sections.is_empty());
        let headings: Vec<&str> = sections.iter().map(|s| s.heading.as_str()).collect();
        assert!(headings.contains(&"Introduction"));
        assert!(headings.contains(&"Background"));
    }

    // ── SerpOrder interleave test ────────────────────────────────────────

    #[test]
    fn test_serp_order_interleaves_queries() {
        let results = vec![
            ("q1".into(), make_result("https://q1r1.com", 1, "s1", 0.5)),
            ("q1".into(), make_result("https://q1r2.com", 2, "s2", 0.5)),
            ("q2".into(), make_result("https://q2r1.com", 1, "s3", 0.5)),
        ];
        let ranked = rank_results(results, &RankStrategy::SerpOrder);
        // q1r1, q2r1, q1r2 (round-robin)
        assert_eq!(ranked.len(), 3);
        assert_eq!(ranked[0].1.url, "https://q1r1.com");
        assert_eq!(ranked[1].1.url, "https://q2r1.com");
        assert_eq!(ranked[2].1.url, "https://q1r2.com");
    }

    // ── DomainAuthority ranking test ─────────────────────────────────────

    #[test]
    fn test_domain_authority_ranking() {
        let results = vec![
            ("q1".into(), make_result("https://low.com", 1, "s", 0.3)),
            ("q1".into(), make_result("https://high.com", 2, "s", 0.9)),
            ("q1".into(), make_result("https://med.com", 3, "s", 0.6)),
        ];
        let ranked = rank_results(results, &RankStrategy::DomainAuthority);
        assert_eq!(ranked[0].1.url, "https://high.com");
        assert_eq!(ranked[1].1.url, "https://med.com");
        assert_eq!(ranked[2].1.url, "https://low.com");
    }

    // ── Multi-query provenance tests ──────────────────────────────────────

    fn make_result_with_source(
        url: &str,
        position: usize,
        snippet: &str,
        authority: f32,
        source_url: &str,
    ) -> SearchResult {
        SearchResult {
            title: format!("Title {position}"),
            url: url.to_string(),
            snippet: snippet.to_string(),
            position,
            provenance: Provenance {
                source_url: source_url.into(),
                method: ExtractionMethod::Search,
                agent: gthings_common::GTHINGS_AGENT.into(),
                accessed_at: Utc::now(),
                duration_ms: 100,
                derived_from: None,
            },
            domain_authority: authority,
        }
    }

    #[test]
    fn test_provenance_source_urls_distinct_per_query() {
        let q1 = "rust";
        let q2 = "tokio";

        fn src(q: &str) -> String {
            format!("https://google.com/search?q={q}")
        }

        let results = vec![
            (
                q1.into(),
                make_result_with_source(
                    "https://example.com/rust1",
                    1,
                    "first rust result",
                    0.5,
                    &src(q1),
                ),
            ),
            (
                q1.into(),
                make_result_with_source(
                    "https://example.com/rust2",
                    2,
                    "second rust result",
                    0.6,
                    &src(q1),
                ),
            ),
            (
                q2.into(),
                make_result_with_source(
                    "https://example.com/tokio1",
                    1,
                    "tokio intro",
                    0.7,
                    &src(q2),
                ),
            ),
            (
                q2.into(),
                make_result_with_source(
                    "https://example.com/tokio2",
                    2,
                    "tokio advanced",
                    0.8,
                    &src(q2),
                ),
            ),
        ];

        let deduped = dedup_results(results, &DedupStrategy::UrlOnly);
        let ranked = rank_results(deduped, &RankStrategy::SerpOrder);

        // Both queries must be represented in the output
        let queries: HashSet<&str> = ranked.iter().map(|(q, _)| q.as_str()).collect();
        assert!(queries.contains("rust"), "missing q1 results");
        assert!(queries.contains("tokio"), "missing q2 results");

        // Each result's provenance.source_url must contain its originating query
        for (query, result) in &ranked {
            assert!(
                result.provenance.source_url.contains(query),
                "source_url '{}' does not contain query '{}'",
                result.provenance.source_url,
                query,
            );
        }
    }

    #[test]
    fn test_parallel_search_collects_all_queries() {
        let results = vec![
            ("q1".into(), make_result("https://a.com/1", 1, "a1", 0.5)),
            ("q1".into(), make_result("https://a.com/2", 2, "a2", 0.5)),
            ("q1".into(), make_result("https://a.com/3", 3, "a3", 0.5)),
            ("q2".into(), make_result("https://b.com/1", 1, "b1", 0.5)),
            ("q2".into(), make_result("https://b.com/2", 2, "b2", 0.5)),
            ("q2".into(), make_result("https://b.com/3", 3, "b3", 0.5)),
            ("q3".into(), make_result("https://c.com/1", 1, "c1", 0.5)),
            ("q3".into(), make_result("https://c.com/2", 2, "c2", 0.5)),
            ("q3".into(), make_result("https://c.com/3", 3, "c3", 0.5)),
        ];

        let deduped = dedup_results(results, &DedupStrategy::UrlOnly);
        assert_eq!(deduped.len(), 9, "dedup should keep all 9 unique URLs");

        let ranked = rank_results(deduped, &RankStrategy::SerpOrder);
        assert_eq!(ranked.len(), 9, "rank should preserve the count");

        // Verify all 9 URLs are unique
        let urls: HashSet<&str> = ranked.iter().map(|(_, r)| r.url.as_str()).collect();
        assert_eq!(urls.len(), 9, "all 9 URLs must be distinct");
    }

    #[test]
    fn test_follow_failure_preserves_search_hits() {
        let sr = make_result("https://example.com/page", 1, "snippet", 0.5);
        let provenance = sr.provenance.clone();

        // Simulate a followed result (success)
        let success = HarvestedResult {
            provenance: provenance.clone(),
            search_result: sr.clone(),
            followed_content: Some("Full content here".into()),
            pagination: None,
            quality: Some(QualityScore {
                score: 0.9,
                is_ok: true,
                reasons: vec![],
                entropy_bits_per_char: 3.5,
            }),
            sections: vec![],
            url_canonical: "https://example.com/page".into(),
            query: "test".into(),
            body_status: BodyStatus::Ok,
        };

        // Simulate a follow failure (timeout/error) — followed_content is None
        let failure = HarvestedResult {
            provenance: provenance.clone(),
            search_result: sr.clone(),
            followed_content: None,
            pagination: None,
            quality: Some(QualityScore {
                score: 0.0,
                is_ok: false,
                reasons: vec!["follow timed out".into()],
                entropy_bits_per_char: 0.0,
            }),
            sections: vec![],
            url_canonical: "https://example.com/page".into(),
            query: "test".into(),
            body_status: BodyStatus::ExtractFailed,
        };

        let results = vec![success, failure];
        assert_eq!(
            results.len(),
            2,
            "follow failure must still be present in output"
        );

        let has_none = results.iter().any(|r| r.followed_content.is_none());
        assert!(has_none, "result with followed_content=None must exist");

        let has_some = results.iter().any(|r| r.followed_content.is_some());
        assert!(has_some, "result with followed_content=Some must exist");
    }

    #[test]
    fn test_empty_query_list() {
        let empty: Vec<(String, SearchResult)> = vec![];
        let deduped = dedup_results(empty.clone(), &DedupStrategy::UrlOnly);
        assert!(deduped.is_empty(), "dedup of empty list must be empty");

        let ranked = rank_results(empty, &RankStrategy::SerpOrder);
        assert!(ranked.is_empty(), "rank of empty list must be empty");
    }

    #[test]
    fn test_single_query_count() {
        let results = vec![
            (
                "q1".into(),
                make_result("https://example.com/1", 1, "first", 0.5),
            ),
            (
                "q1".into(),
                make_result("https://example.com/2", 2, "second", 0.5),
            ),
            (
                "q1".into(),
                make_result("https://example.com/3", 3, "third", 0.5),
            ),
            (
                "q1".into(),
                make_result("https://example.com/4", 4, "fourth", 0.5),
            ),
            (
                "q1".into(),
                make_result("https://example.com/5", 5, "fifth", 0.5),
            ),
        ];
        let deduped = dedup_results(results.clone(), &DedupStrategy::UrlOnly);
        assert_eq!(deduped.len(), 5, "5 results should stay 5 after dedup");

        let ranked = rank_results(results, &RankStrategy::SerpOrder);
        assert_eq!(ranked.len(), 5, "5 results should stay 5 after rank");
    }

    // ── Strategy-lock tests (Level 2) ──────────────────────────────────

    #[test]
    fn test_dedup_fragment_urls_collapse_to_one() {
        // URLs differing only by fragment → same dedup_key → single candidate
        let results = vec![
            (
                "q1".into(),
                make_result("https://example.com/page#:~:text=hello", 1, "s1", 0.5),
            ),
            (
                "q1".into(),
                make_result("https://example.com/page#:~:text=world", 2, "s2", 0.5),
            ),
            (
                "q1".into(),
                make_result("https://example.com/page#section1", 3, "s3", 0.5),
            ),
        ];
        let selected = select_follow_candidates(results, 10);
        assert_eq!(
            selected.len(),
            1,
            "three URLs with same base but different fragments should collapse to one candidate"
        );
        assert_eq!(
            selected[0].1.url, "https://example.com/page#:~:text=hello",
            "first-occurrence URL should be kept"
        );
    }

    #[test]
    fn test_select_follow_candidates_per_query_minimum() {
        // 3 queries, each with 3 results, all distinct domains, follow_max=4
        // Should include ≥1 from each query
        let results = vec![
            (
                "q1".into(),
                make_result("https://alpha.com/a", 1, "s1", 0.5),
            ),
            ("q1".into(), make_result("https://beta.com/b", 2, "s2", 0.5)),
            (
                "q1".into(),
                make_result("https://gamma.com/c", 3, "s3", 0.5),
            ),
            (
                "q2".into(),
                make_result("https://delta.com/a", 1, "s4", 0.5),
            ),
            (
                "q2".into(),
                make_result("https://epsilon.com/b", 2, "s5", 0.5),
            ),
            ("q2".into(), make_result("https://zeta.com/c", 3, "s6", 0.5)),
            ("q3".into(), make_result("https://eta.com/a", 1, "s7", 0.5)),
            (
                "q3".into(),
                make_result("https://theta.com/b", 2, "s8", 0.5),
            ),
            ("q3".into(), make_result("https://iota.com/c", 3, "s9", 0.5)),
        ];
        let selected = select_follow_candidates(results, 4);
        let queries: HashSet<&str> = selected.iter().map(|(q, _)| q.as_str()).collect();
        assert!(
            queries.contains("q1"),
            "q1 must have at least one candidate"
        );
        assert!(
            queries.contains("q2"),
            "q2 must have at least one candidate"
        );
        assert!(
            queries.contains("q3"),
            "q3 must have at least one candidate"
        );
        assert_eq!(
            selected.len(),
            4,
            "follow_max=4 should return exactly 4 candidates"
        );
    }

    #[test]
    fn test_select_follow_candidates_per_host_cap() {
        // 6 results all from same domain, max=3 (less than 6 so domain cap applies)
        // Domain cap of 2 means at most 2 should be selected
        let results = vec![
            (
                "q1".into(),
                make_result("https://example.com/a", 1, "s1", 0.5),
            ),
            (
                "q1".into(),
                make_result("https://example.com/b", 2, "s2", 0.5),
            ),
            (
                "q1".into(),
                make_result("https://example.com/c", 3, "s3", 0.5),
            ),
            (
                "q1".into(),
                make_result("https://example.com/d", 4, "s4", 0.5),
            ),
            (
                "q1".into(),
                make_result("https://example.com/e", 5, "s5", 0.5),
            ),
            (
                "q1".into(),
                make_result("https://example.com/f", 6, "s6", 0.5),
            ),
        ];
        // Use max=3 — less than deduped count (6), so domain cap logic activates
        let selected = select_follow_candidates(results, 3);
        assert!(
            selected.len() <= 2,
            "at most 2 results from same domain (domain cap), got {}",
            selected.len()
        );
    }

    #[test]
    fn test_select_follow_candidates_filters_junk() {
        // URLs matching accounts.google.com, support.google.com etc. should be filtered out
        let results = vec![
            (
                "q1".into(),
                make_result("https://accounts.google.com/signin", 1, "s1", 0.5),
            ),
            (
                "q1".into(),
                make_result("https://support.google.com/help", 2, "s2", 0.5),
            ),
            (
                "q1".into(),
                make_result("https://policies.google.com/privacy", 3, "s3", 0.5),
            ),
            (
                "q1".into(),
                make_result("https://example.com/real-content", 4, "s4", 0.5),
            ),
        ];
        let selected = select_follow_candidates(results, 10);
        assert_eq!(
            selected.len(),
            1,
            "only the non-junk URL should survive filtering"
        );
        assert_eq!(
            selected[0].1.url, "https://example.com/real-content",
            "the real content URL should be selected"
        );
    }

    #[test]
    fn test_compute_quality_reasons_never_empty_when_low() {
        // Empty content → reasons has "empty_content"
        let q = compute_quality("");
        assert!(
            q.reasons.iter().any(|r| r == "empty_content"),
            "empty content should produce 'empty_content' reason, got: {:?}",
            q.reasons
        );

        // Bot wall → reasons has "bot_blocked"
        let bot_text = "Checking your browser before accessing the site. Please wait while we verify you are human.";
        let q = compute_quality(bot_text);
        assert!(
            q.reasons.iter().any(|r| r.contains("bot_blocked")),
            "bot-detected content should produce 'bot_blocked' reason, got: {:?}",
            q.reasons
        );

        // Paywall → reasons has "paywall"
        let paywall_text = "Subscribe now to continue reading this article. You have reached your free article limit.";
        let q = compute_quality(paywall_text);
        assert!(
            q.reasons.iter().any(|r| r.contains("paywall")),
            "paywall content should produce 'paywall' reason, got: {:?}",
            q.reasons
        );

        // Tiny content → reasons has "too_short"
        let q = compute_quality("tiny");
        assert!(
            q.reasons.iter().any(|r| r.contains("too_short")),
            "tiny content should produce 'too_short' reason, got: {:?}",
            q.reasons
        );
    }

    #[test]
    fn test_phase_follow_marks_pdf_as_unextracted() {
        // PDF URLs are detected and mapped to PdfUnextracted with no followed content
        assert!(
            is_pdf_url("https://example.com/doc.pdf"),
            ".pdf URL must be detected as PDF"
        );
        assert!(
            is_pdf_url("https://arxiv.org/pdf/2301.00001"),
            "arxiv.org/pdf/... must be detected as PDF"
        );
        assert!(
            is_arxiv_url("https://arxiv.org/abs/2301.00001"),
            "arxiv.org/abs/... must be detected as arXiv"
        );
        assert!(
            is_arxiv_url("https://arxiv.org/pdf/2301.00001"),
            "arxiv.org/pdf/... must be detected as arXiv"
        );

        // Construct what phase_follow produces for a PDF URL
        let sr = make_result("https://example.com/doc.pdf", 1, "s", 0.5);
        let pdf_result = HarvestedResult {
            provenance: sr.provenance.clone(),
            search_result: sr,
            followed_content: None,
            pagination: None,
            quality: None,
            sections: Vec::new(),
            url_canonical: "https://example.com/doc.pdf".into(),
            query: "test".into(),
            body_status: BodyStatus::PdfUnextracted,
        };
        assert!(
            pdf_result.followed_content.is_none(),
            "PDF results should have no followed content (CDP skipped)"
        );
        assert!(
            matches!(pdf_result.body_status, BodyStatus::PdfUnextracted),
            "PDF results should have body_status PdfUnextracted"
        );
    }

    #[test]
    fn test_harvest_run_summary_coverage() {
        // Build HarvestedResult instances with varied body_status per query
        let sr1 = make_result("https://a.com", 1, "s", 0.5);
        let sr2 = make_result("https://b.com", 2, "s", 0.5);
        let sr3 = make_result("https://c.com", 3, "s", 0.5);
        let sr4 = make_result("https://d.com", 4, "s", 0.5);
        let sr5 = make_result("https://e.com", 5, "s", 0.5);

        let ok_quality = || QualityScore {
            score: 1.0,
            is_ok: true,
            reasons: vec![],
            entropy_bits_per_char: 4.0,
        };

        let results = vec![
            HarvestedResult {
                provenance: sr1.provenance.clone(),
                search_result: sr1,
                followed_content: Some("content".into()),
                pagination: None,
                quality: Some(ok_quality()),
                sections: vec![],
                url_canonical: "https://a.com".into(),
                query: "rust".into(),
                body_status: BodyStatus::Ok,
            },
            HarvestedResult {
                provenance: sr2.provenance.clone(),
                search_result: sr2,
                followed_content: None,
                pagination: None,
                quality: None,
                sections: vec![],
                url_canonical: "https://b.com".into(),
                query: "rust".into(),
                body_status: BodyStatus::ExtractFailed,
            },
            HarvestedResult {
                provenance: sr3.provenance.clone(),
                search_result: sr3,
                followed_content: None,
                pagination: None,
                quality: None,
                sections: vec![],
                url_canonical: "https://c.com".into(),
                query: "tokio".into(),
                body_status: BodyStatus::ExtractFailed,
            },
            HarvestedResult {
                provenance: sr4.provenance.clone(),
                search_result: sr4,
                followed_content: None,
                pagination: None,
                quality: None,
                sections: vec![],
                url_canonical: "https://d.com".into(),
                query: "tokio".into(),
                body_status: BodyStatus::ExtractFailed,
            },
            HarvestedResult {
                provenance: sr5.provenance.clone(),
                search_result: sr5,
                followed_content: None,
                pagination: None,
                quality: None,
                sections: vec![],
                url_canonical: "https://e.com".into(),
                query: "nightmare".into(),
                body_status: BodyStatus::ExtractFailed,
            },
        ];

        // Build coverage (replicating harvest() logic)
        let mut coverage: HashMap<String, QueryCoverage> = HashMap::new();
        for r in &results {
            let entry = coverage.entry(r.query.clone()).or_insert(QueryCoverage {
                total_hits: 0,
                followed_ok: 0,
                followed_failed: 0,
            });
            entry.total_hits += 1;
            match r.body_status {
                BodyStatus::Ok => entry.followed_ok += 1,
                _ => entry.followed_failed += 1,
            }
        }

        // Verify coverage counts
        let rust_cov = coverage.get("rust").expect("rust coverage entry");
        assert_eq!(rust_cov.total_hits, 2, "rust has 2 results");
        assert_eq!(rust_cov.followed_ok, 1, "rust has 1 OK result");
        assert_eq!(rust_cov.followed_failed, 1, "rust has 1 failed result");

        let tokio_cov = coverage.get("tokio").expect("tokio coverage entry");
        assert_eq!(tokio_cov.total_hits, 2, "tokio has 2 results");
        assert_eq!(tokio_cov.followed_ok, 0, "tokio has 0 OK results");

        let nightmare_cov = coverage.get("nightmare").expect("nightmare coverage entry");
        assert_eq!(nightmare_cov.total_hits, 1, "nightmare has 1 result");
        assert_eq!(nightmare_cov.followed_ok, 0, "nightmare has 0 OK results");

        // Verify warnings for queries with no OK result
        let warnings: Vec<HarvestWarning> = coverage
            .iter()
            .filter(|(_, c)| c.followed_ok == 0 && c.total_hits > 0)
            .map(|(q, _)| HarvestWarning::NoBodyForQuery(q.clone()))
            .collect();
        assert_eq!(
            warnings.len(),
            2,
            "tokio and nightmare should generate NoBodyForQuery warnings"
        );
    }

    #[test]
    fn test_body_status_mapping() {
        // Empty content → ChromeOrEmpty (indirectly via quality)
        let q = compute_quality("");
        assert!(!q.is_ok, "empty content quality should not be ok");
        assert!(
            q.reasons.iter().any(|r| r == "empty_content"),
            "empty content should have empty_content reason"
        );

        // Good content → Ok (mapped via quality check)
        let good = "This is a sufficiently long piece of content with many words \
                     and sentences that should pass all quality checks without \
                     triggering any of the detection heuristics.";
        let q = compute_quality(good);
        assert!(q.is_ok, "good content quality should be ok");

        // PDF URL → PdfUnextracted (detected before follow in phase_follow)
        assert!(
            is_pdf_url("https://example.com/report.pdf"),
            "URL ending in .pdf must be detected as PDF"
        );
        assert!(
            matches!(BodyStatus::PdfUnextracted, BodyStatus::PdfUnextracted),
            "PdfUnextracted variant exists and is constructable"
        );
    }

    #[test]
    fn test_all_queries_represented_in_output() {
        // select_follow_candidates preserves query provenance for all input queries
        let results = vec![
            (
                "rust".into(),
                make_result("https://rust-lang.org", 1, "s1", 0.9),
            ),
            (
                "tokio".into(),
                make_result("https://tokio.rs", 1, "s2", 0.7),
            ),
            (
                "actix".into(),
                make_result("https://actix.rs", 1, "s3", 0.6),
            ),
        ];
        let selected = select_follow_candidates(results, 10);
        let queries: HashSet<&str> = selected.iter().map(|(q, _)| q.as_str()).collect();
        assert!(
            queries.contains("rust"),
            "rust query must be represented in output"
        );
        assert!(
            queries.contains("tokio"),
            "tokio query must be represented in output"
        );
        assert!(
            queries.contains("actix"),
            "actix query must be represented in output"
        );
        assert_eq!(selected.len(), 3, "all 3 results should be selected");
    }

    #[test]
    fn test_select_follow_candidates_empty() {
        let selected = select_follow_candidates(vec![], 10);
        assert!(selected.is_empty(), "empty input must produce empty output");

        let selected = select_follow_candidates(vec![], 0);
        assert!(
            selected.is_empty(),
            "empty input with max=0 must produce empty output"
        );
    }

    #[test]
    fn test_select_follow_candidates_single_domain_warning() {
        // All candidates from same domain → FollowBudgetCollapsedToOneSite condition
        let results = vec![
            (
                "q1".into(),
                make_result("https://example.com/a", 1, "s1", 0.5),
            ),
            (
                "q1".into(),
                make_result("https://example.com/b", 2, "s2", 0.5),
            ),
            (
                "q1".into(),
                make_result("https://example.com/c", 3, "s3", 0.5),
            ),
            (
                "q2".into(),
                make_result("https://example.com/d", 1, "s4", 0.5),
            ),
        ];
        // Use max=3 (< deduped count of 4) so domain-cap logic activates
        let selected = select_follow_candidates(results, 3);

        // Replicate harvest() warning logic
        let mut domains_selected: HashSet<String> = HashSet::new();
        for (_, r) in &selected {
            if let Some(d) = registered_domain(&r.url) {
                domains_selected.insert(d);
            }
        }
        assert!(
            domains_selected.len() <= 1 && selected.len() > 1,
            "all selected from same domain with >1 selected should trigger warning condition"
        );
        assert!(
            selected.len() <= 2,
            "domain cap limits to at most 2 per domain, got {}",
            selected.len()
        );
    }

    // ── normalize_url direct tests ─────────────────────────────────────

    #[test]
    fn test_normalize_url_case_and_slash() {
        assert_eq!(
            normalize_url("HTTP://EXAMPLE.COM/Path/"),
            "http://example.com/path"
        );
    }

    #[test]
    fn test_normalize_url_strips_tracking() {
        let result = normalize_url("https://example.com/page?utm_source=test&keep=1&fbclid=abc");
        assert!(!result.contains("utm_source"), "should strip utm_source");
        assert!(!result.contains("fbclid"), "should strip fbclid");
        assert!(result.contains("keep=1"), "should keep non-tracking params");
    }

    #[test]
    fn test_normalize_url_invalid_passthrough() {
        assert_eq!(normalize_url("not a url"), "not a url");
        assert_eq!(normalize_url(""), "");
    }

    #[test]
    fn test_normalize_url_preserves_fragment() {
        let result = normalize_url("https://example.com/page#section1");
        assert!(result.contains("#section1"), "should preserve fragment");
    }

    // ── is_junk_url direct tests ───────────────────────────────────────

    #[test]
    fn test_is_junk_url_detection() {
        // These should be junk
        assert!(is_junk_url("https://accounts.google.com/signin"));
        assert!(is_junk_url("https://support.google.com/websearch"));
        assert!(is_junk_url("https://policies.google.com/privacy"));
        assert!(is_junk_url("https://example.com/track?foo=1"));
        assert!(is_junk_url("https://doubleclick.net/ads"));
        assert!(is_junk_url("https://pagead2.googlesyndication.com/test"));
        // These should NOT be junk
        assert!(!is_junk_url("https://en.wikipedia.org/wiki/Entropy"));
        assert!(!is_junk_url("https://arxiv.org/abs/2112.06034"));
        assert!(!is_junk_url("https://example.com/page"));
    }

    // ── Composite rank correctness ─────────────────────────────────────

    #[test]
    fn test_composite_rank_snippet_over_position() {
        // A lower-position result with a long snippet should outrank a
        // higher-position result with a short snippet, given equal authority.
        let results = vec![
            (
                "q1".into(),
                make_result("https://example.com/short", 1, "Short snip", 0.9),
            ),
            (
                "q1".into(),
                make_result(
                    "https://example.com/long",
                    2,
                    "This is a much longer snippet that provides more information to the user for ranking purposes",
                    0.9,
                ),
            ),
        ];
        let ranked = rank_results(results, &RankStrategy::Composite);
        assert_eq!(ranked.len(), 2);
        // The long-snippet result (position 2) should rank first in composite mode
        assert_eq!(
            ranked[0].1.url, "https://example.com/long",
            "long snippet should outrank short snippet in composite mode"
        );
    }
}
