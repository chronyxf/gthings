use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;

use crate::SearchResult;
use gthings_common::url_normalizer::dedup_key;

use super::types::{DedupStrategy, RankStrategy};

/// Deduplicate search results, keeping the first occurrence of each
/// normalized URL.
pub(super) fn dedup_results(
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

/// Rank deduplicated results according to the chosen strategy.
pub(super) fn rank_results(
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
                .unwrap_or(1);

            // Count how many queries returned each normalized URL
            let mut url_query_count: HashMap<String, usize> = HashMap::new();
            for (_, r) in &results {
                let norm = dedup_key(&r.url);
                *url_query_count.entry(norm).or_insert(0) += 1;
            }

            let mut scored: Vec<(f64, usize, String, SearchResult)> = results
                .into_iter()
                .enumerate()
                .map(|(idx, (query, r))| {
                    let norm = dedup_key(&r.url);
                    let query_count = *url_query_count.get(&norm).unwrap_or(&1);
                    let diversity_bonus = 1.0 / query_count as f64;
                    let norm_snippet = r.snippet.len() as f64 / max_snippet_len as f64;
                    let score = 0.5 * r.domain_authority
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
    // Group results by query, preserving insertion order via IndexMap
    let mut by_query: IndexMap<String, Vec<(String, SearchResult)>> = IndexMap::new();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SearchResult;
    use chrono::Utc;
    use gthings_common::provenance::{ExtractionMethod, Provenance};

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
            domain_authority: authority as f64,
            source_type: "web".into(),
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

    // ── Additional rank/dedup tests ──────────────────────────────────────

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
