//! Follow selection with diversity and Phase 3: parallel follow.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gthings_cdp::{Session, TabGuard};
use gthings_common::domain_reputation::DomainReputation;
use gthings_common::pagination::ExtractParams;
use gthings_common::provenance::Provenance;
use gthings_common::url_normalizer::{
    canonicalize_url, dedup_key, is_arxiv_url, is_pdf_url, registered_domain,
};
use gthings_extraction::article::QualityScore;
use tokio::task::JoinSet;

use super::super::quality::{compute_quality_with_flags, extract_sections, is_nav_heavy};
use super::super::types::*;
use super::junk::{is_junk_title, is_junk_url};
use super::search::map_join_err;
use crate::SearchResult;

/// Classify [`BodyStatus`] from a [`FollowResult`] and its [`QualityScore`].
pub(crate) fn classify_body_status(
    fr: &crate::follow::FollowResult,
    quality: &QualityScore,
) -> BodyStatus {
    if fr.content.is_empty() && fr.error.is_empty() {
        BodyStatus::ChromeOrEmpty
    } else if !fr.error.is_empty() {
        BodyStatus::ExtractFailed
    } else if !quality.is_ok
        && quality
            .reasons
            .iter()
            .any(|r| r.contains("too_short") || r.contains("nav") || r.contains("chrome"))
        && is_nav_heavy(&fr.content)
    {
        // Only drop as ChromeOrEmpty when the content is genuinely pure
        // navigation chrome. Dense/repetitive raw prose is never nav-heavy,
        // so it is always kept even if it carries some boilerplate or low
        // entropy.
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
// Follow selection with diversity
// ---------------------------------------------------------------------------

/// Select up to `max` URLs with diversity constraints:
/// 1. Dedup on dedup_key() from url_normalizer
/// 2. Per-query minimum — try include ≥1 hit from each query
/// 3. Per-canonical-host cap — max 2 follows per registered domain
/// 4. Remove known junk (google.com/accounts/*, google.com/support/*, etc.)
///
/// Returns the selected candidates together with the set of registered domains
/// among them (computed in the same pass, so callers need not recompute).
pub(crate) fn select_follow_candidates(
    ranked: Vec<(String, SearchResult)>,
    max: usize,
) -> (Vec<(String, SearchResult)>, HashSet<String>) {
    if max == 0 || ranked.is_empty() {
        return (Vec::new(), HashSet::new());
    }

    // Stage 1: dedup on dedup_key() and remove known junk
    let mut seen_key = HashSet::new();
    let deduped: Vec<(String, SearchResult)> = ranked
        .into_iter()
        .filter(|(_, r)| {
            !is_junk_url(&r.url) && !is_junk_title(&r.title) && seen_key.insert(dedup_key(&r.url))
        })
        .collect();

    if max >= deduped.len() {
        let domains = deduped
            .iter()
            .filter_map(|(_, r)| registered_domain(&r.url))
            .collect();
        return (deduped, domains);
    }

    // Stage 2: group by query for diversity picking. Each item carries its
    // registered domain precomputed ONCE so we never recompute it per pick.
    let mut by_query: HashMap<String, Vec<(String, SearchResult, String)>> = HashMap::new();
    for (q, r) in deduped {
        let domain = registered_domain(&r.url).unwrap_or_default();
        by_query.entry(q.clone()).or_default().push((q, r, domain));
    }
    let query_keys: Vec<String> = by_query.keys().cloned().collect();

    let mut selected: Vec<(String, SearchResult)> = Vec::with_capacity(max);
    let mut selected_domains: HashSet<String> = HashSet::new();
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
                if let Some((q, r, domain)) = pick_one(items, &mut domain_count) {
                    selected_domains.insert(domain);
                    selected.push((q, r));
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
            while let Some((q, r, domain)) = pick_one(items, &mut domain_count) {
                if selected.len() >= max {
                    break;
                }
                selected_domains.insert(domain);
                selected.push((q, r));
            }
        }
    }

    (selected, selected_domains)
}

/// Pick one item from `items` whose domain is under the per-domain cap.
/// Returns `None` if no eligible item remains. Uses `swap_remove` to avoid the
/// O(n) shift of `remove(pos)`.
fn pick_one(
    items: &mut Vec<(String, SearchResult, String)>,
    domain_count: &mut HashMap<String, usize>,
) -> Option<(String, SearchResult, String)> {
    let pos = items
        .iter()
        .position(|(_, _, domain)| *domain_count.get(domain).unwrap_or(&0) < 2)?;
    let item = items.swap_remove(pos);
    *domain_count.entry(item.2.clone()).or_insert(0) += 1;
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
        provenance: crate::follow::error_provenance(&url, 0),
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
pub(crate) async fn phase_follow(
    session: Arc<Session>,
    ranked: Vec<(String, SearchResult)>,
    follow_top_n: usize,
    params: ExtractParams,
    reputation: Option<Arc<DomainReputation>>,
) -> Vec<HarvestedResult> {
    let mut harvested: Vec<HarvestedResult> = Vec::with_capacity(ranked.len());
    let follow_count = follow_top_n.min(ranked.len());
    let mut follow_join_set: JoinSet<(usize, HarvestedResult)> = JoinSet::new();

    // Cap concurrent CDP tabs so they open in waves rather than all at once.
    let follow_semaphore = Arc::new(tokio::sync::Semaphore::new(
        crate::engine::MAX_CONCURRENT_TABS,
    ));

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
            let semaphore = Arc::clone(&follow_semaphore);

            follow_join_set.spawn(async move {
                // Acquire a permit before opening a tab; released when the task ends.
                // Bound the wait so queued tasks give up rather than waiting
                // unboundedly behind the 4-permit cap.
                let _permit =
                    match super::acquire_permit(semaphore, crate::engine::OP_TIMEOUT, || {
                        (
                            idx,
                            make_error_result(
                                search_result.clone(),
                                url.clone(),
                                url_canonical.clone(),
                                q_task.clone(),
                                "follow semaphore acquire failed",
                            ),
                        )
                    })
                    .await
                    {
                        Ok(permit) => permit,
                        Err(fail) => return fail,
                    };
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

                // RAII guard: closes the tab on ALL exit paths, including task
                // cancellation/abort. The guard drops when the task ends.
                let _guard = TabGuard::new(&session, tab.clone());

                let result = tokio::time::timeout(
                    crate::engine::OP_TIMEOUT,
                    crate::follow::follow(&session, &tab, &url, params, rep.as_deref()),
                )
                .await;

                let harvest_result = match result {
                    Ok(Ok(fr)) => {
                        let skip_len = matches!(
                            fr.provenance.method,
                            gthings_common::provenance::ExtractionMethod::Pdf
                                | gthings_common::provenance::ExtractionMethod::Arxiv
                        );
                        let quality =
                            compute_quality_with_flags(&fr.content, skip_len, &fr.quality_flags);
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
