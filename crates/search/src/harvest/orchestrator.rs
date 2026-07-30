//! Orchestration: phase_search, phase_follow, harvest, and helpers.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use gthings_cdp::{CdpError, Session};
use gthings_common::domain_reputation::DomainReputation;
use gthings_common::pagination::ExtractParams;
use gthings_common::provenance::{ExtractionMethod, Provenance};
use gthings_common::url_normalizer::{
    canonicalize_url, dedup_key, is_arxiv_url, is_pdf_url, registered_domain,
};
use gthings_extraction::article::QualityScore;
use tokio::task::JoinSet;

use crate::SearchResult;
use crate::follow::TimedSearchOutcome;

use super::quality::{compute_quality, extract_sections};
use super::ranking::{dedup_results, rank_results};
use super::types::*;

/// Convert a [`tokio::task::JoinError`] into a [`CdpError`].
fn map_join_err(method: &str, err: tokio::task::JoinError) -> CdpError {
    CdpError::CdpCallFailed {
        method: method.into(),
        detail: format!("join error: {err}"),
    }
}

/// Classify [`BodyStatus`] from a [`FollowResult`] and its [`QualityScore`].
fn classify_body_status(fr: &crate::follow::FollowResult, quality: &QualityScore) -> BodyStatus {
    if fr.content.is_empty() && fr.error.is_empty() {
        BodyStatus::ChromeOrEmpty
    } else if !fr.error.is_empty() {
        BodyStatus::ExtractFailed
    } else if quality.score < 0.3
        && quality
            .reasons
            .iter()
            .any(|r| r.contains("too_short") || r.contains("nav") || r.contains("chrome"))
    {
        BodyStatus::ChromeOrEmpty
    } else if !quality.is_ok
        && quality
            .reasons
            .iter()
            .any(|r| r.contains("paywall") || r.contains("bot_blocked") || r.contains("captcha"))
    {
        BodyStatus::ExtractFailed
    } else {
        BodyStatus::Ok
    }
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
            // Use shared helper: create tab → timed search → close tab
            match crate::follow::search_with_tab(&session, &query, count, search_timeout).await {
                Ok(TimedSearchOutcome::Success(results)) => Ok((query, results)),
                Ok(TimedSearchOutcome::Error(e)) => Err(e),
                Ok(TimedSearchOutcome::Timeout) => {
                    tracing::warn!("harvest search timed out for query: {query}");
                    Ok((query, Vec::new()))
                }
                Err(e) => Err(e),
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

// ---------------------------------------------------------------------------
// Junk URL filtering
// ---------------------------------------------------------------------------

/// Junk URL patterns to exclude from follow.
pub(crate) fn is_junk_url(url: &str) -> bool {
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

// ---------------------------------------------------------------------------
// Follow selection with diversity
// ---------------------------------------------------------------------------

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
                if let Some(item) = pick_one(items, &mut domain_count) {
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
            while let Some(item) = pick_one(items, &mut domain_count) {
                if selected.len() >= max {
                    break;
                }
                selected.push(item);
            }
        }
    }

    selected
}

/// Pick one item from `items` whose domain is under the per-domain cap.
/// Returns `None` if no eligible item remains.
fn pick_one(
    items: &mut Vec<(String, SearchResult)>,
    domain_count: &mut HashMap<String, usize>,
) -> Option<(String, SearchResult)> {
    let pos = items.iter().position(|(_, r)| {
        let domain = registered_domain(&r.url).unwrap_or_default();
        *domain_count.get(&domain).unwrap_or(&0) < 2
    })?;
    let item = items.remove(pos);
    let domain = registered_domain(&item.1.url).unwrap_or_default();
    *domain_count.entry(domain).or_insert(0) += 1;
    Some(item)
}

// ---------------------------------------------------------------------------
// Helper: build an error result for follow failures
// ---------------------------------------------------------------------------

/// Build a skeleton `HarvestedResult` for the snippet-only / unextracted case
/// (no followed content, no quality assessment).
fn make_snippet_only_result(
    provenance: Provenance,
    search_result: SearchResult,
    url_canonical: String,
    query: String,
    body_status: BodyStatus,
) -> HarvestedResult {
    HarvestedResult {
        provenance,
        search_result,
        followed_content: None,
        pagination: None,
        quality: None,
        sections: Vec::new(),
        url_canonical,
        query,
        body_status,
    }
}

fn make_error_result(
    search_result: SearchResult,
    url: String,
    url_canonical: String,
    query: String,
    reason: &str,
) -> HarvestedResult {
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
            reasons: vec![reason.to_string()],
            entropy_bits_per_char: 0.0,
        }),
        sections: Vec::new(),
        url_canonical,
        query,
        body_status: BodyStatus::ExtractFailed,
    }
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
        let url_canonical = canonicalize_url(&search_result.url);
        let q = query.clone();

        if idx < follow_count {
            // Check if URL is PDF or arXiv — skip CDP follow, mark as PdfUnextracted
            if is_pdf_url(&search_result.url) || is_arxiv_url(&search_result.url) {
                harvested.push(make_snippet_only_result(
                    search_result.provenance.clone(),
                    search_result,
                    url_canonical,
                    query,
                    BodyStatus::PdfUnextracted,
                ));
                continue;
            }

            // Push placeholder; will be replaced when follow completes
            let placeholder = make_snippet_only_result(
                search_result.provenance.clone(),
                search_result.clone(),
                url_canonical.clone(),
                q.clone(),
                BodyStatus::SnippetOnly,
            );
            harvested.push(placeholder);

            // Spawn follow task
            let session = Arc::clone(&session);
            let url = search_result.url.clone();
            let params = params.clone();
            let rep = rep_outer.clone();
            let q_task = q;

            follow_join_set.spawn(async move {
                let tab = match session.create_background_tab().await {
                    Ok(t) => t,
                    Err(e) => {
                        return (
                            idx,
                            make_error_result(
                                search_result,
                                url,
                                url_canonical,
                                q_task,
                                &format!("create_tab failed: {e}"),
                            ),
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
                        let skip_len = matches!(
                            fr.provenance.method,
                            gthings_common::provenance::ExtractionMethod::Pdf
                                | gthings_common::provenance::ExtractionMethod::Arxiv
                        );
                        let quality = compute_quality(&fr.content, skip_len);
                        let sections = extract_sections(&fr.content);
                        let body_status = classify_body_status(&fr, &quality);
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
                    Ok(Err(e)) => make_error_result(
                        search_result,
                        url,
                        url_canonical,
                        q_task,
                        &format!("follow failed: {e}"),
                    ),
                    Err(_) => make_error_result(
                        search_result,
                        url,
                        url_canonical,
                        q_task,
                        "follow timed out",
                    ),
                };

                (idx, harvest_result)
            });
        } else {
            // Beyond follow_top_n — include without following
            harvested.push(make_snippet_only_result(
                search_result.provenance.clone(),
                search_result,
                url_canonical,
                query,
                BodyStatus::SnippetOnly,
            ));
        }
    }

    // Collect followed results in any order and place at correct index
    while let Some(task_result) = follow_join_set.join_next().await {
        match task_result {
            Ok((idx, result)) => {
                harvested[idx] = result;
            }
            Err(join_err) => {
                tracing::warn!("{}", map_join_err("harvest_follow", join_err));
                // The placeholder remains (followed_content = None, quality = None)
            }
        }
    }

    harvested
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SearchResult;
    use chrono::Utc;
    use gthings_common::provenance::{ExtractionMethod, Provenance};
    use gthings_common::url_normalizer::is_pdf_url;

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

    // ── Multi-query provenance tests ──────────────────────────────────────

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

        let results = [success, failure];
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

    // ── phase_follow PDF tests ──────────────────────────────────────────

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

    // ── HarvestRunSummary coverage tests ────────────────────────────────

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
}
