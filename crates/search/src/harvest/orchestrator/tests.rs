use crate::FollowResult;
use crate::SearchResult;
use crate::engine::{EngineMode, SearchEngine};
use crate::harvest::orchestrator::follow::{classify_body_status, select_follow_candidates};
use crate::harvest::orchestrator::is_junk_url;
use crate::harvest::orchestrator::junk::is_junk_title;
use crate::harvest::quality::compute_quality_with_flags;
use crate::harvest::ranking::{dedup_results, rank_results};
use crate::harvest::types::{BodyStatus, RankStrategy};
use chrono::Utc;
use gthings_common::provenance::{ExtractionMethod, Provenance};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinSet;

fn make_result(url: &str, position: usize, snippet: &str, authority: f32) -> SearchResult {
    SearchResult {
        title: format!("Title {position}"),
        url: url.to_string(),
        snippet: snippet.to_string(),
        position,
        provenance: Provenance {
            source_url: "https://www.google.com/search?q=test".into(),
            method: ExtractionMethod::Search,
            agent: gthings_common::user_agent::gthings_agent(),
            accessed_at: Utc::now(),
            duration_ms: 100,
        },
        domain_authority: authority as f64,
        source_type: "web".into(),
        engine: SearchEngine::Brave,
        score: 0.0,
        published_date: None,
        favicon: None,
        mode: EngineMode::Hybrid,
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
            agent: gthings_common::user_agent::gthings_agent(),
            accessed_at: Utc::now(),
            duration_ms: 100,
        },
        domain_authority: authority as f64,
        source_type: "web".into(),
        engine: SearchEngine::Brave,
        score: 0.0,
        published_date: None,
        favicon: None,
        mode: EngineMode::Hybrid,
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

    let deduped = dedup_results(results);
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

    let deduped = dedup_results(results);
    assert_eq!(deduped.len(), 9, "dedup should keep all 9 unique URLs");

    let ranked = rank_results(deduped, &RankStrategy::SerpOrder);
    assert_eq!(ranked.len(), 9, "rank should preserve the count");

    // Verify all 9 URLs are unique
    let urls: HashSet<&str> = ranked.iter().map(|(_, r)| r.url.as_str()).collect();
    assert_eq!(urls.len(), 9, "all 9 URLs must be distinct");
}

#[tokio::test]
async fn test_search_semaphore_caps_concurrency_at_four() {
    // Mirrors the phase_search concurrency cap: a 4-permit semaphore must
    // limit in-flight searches to 4 even when many tasks are spawned.
    let semaphore = Arc::new(tokio::sync::Semaphore::new(4));
    let tasks = 12;
    let mut join_set: JoinSet<usize> = JoinSet::new();
    let active = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let peak = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    for _ in 0..tasks {
        let semaphore = Arc::clone(&semaphore);
        let active = Arc::clone(&active);
        let peak = Arc::clone(&peak);
        join_set.spawn(async move {
            let _permit = semaphore.acquire().await.expect("semaphore closed");
            let now = active.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            peak.fetch_max(now, std::sync::atomic::Ordering::SeqCst);
            // Simulate a search taking some time.
            tokio::time::sleep(Duration::from_millis(5)).await;
            active.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            1
        });
    }

    let mut total = 0;
    while let Some(res) = join_set.join_next().await {
        total += res.expect("task should not panic");
    }
    assert_eq!(total, tasks, "all tasks must complete");
    assert_eq!(
        peak.load(std::sync::atomic::Ordering::SeqCst),
        4,
        "concurrency must never exceed the 4-permit cap"
    );
}

#[tokio::test]
async fn test_search_semaphore_acquire_times_out() {
    // Mirrors the phase_search acquire-timeout degradation: when a task
    // cannot obtain a permit within the bounded window, it must give up and
    // degrade to an empty result instead of waiting unboundedly.
    let semaphore = Arc::new(tokio::sync::Semaphore::new(1));
    // Hold the only permit so queued tasks cannot acquire.
    let _held = semaphore.clone().acquire_owned().await.unwrap();
    let acquire_timeout = Duration::from_millis(20);

    let mut join_set: JoinSet<Vec<String>> = JoinSet::new();
    for _ in 0..3 {
        let semaphore = Arc::clone(&semaphore);
        join_set.spawn(async move {
            match tokio::time::timeout(acquire_timeout, semaphore.acquire()).await {
                Ok(Ok(_permit)) => vec!["acquired".to_string()],
                Ok(Err(_)) | Err(_) => Vec::new(), // degrade to empty
            }
        });
    }

    let mut total = 0;
    while let Some(res) = join_set.join_next().await {
        total += res.expect("task should not panic").len();
    }
    assert_eq!(
        total, 0,
        "all queued tasks should time out and degrade to empty results"
    );
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
    let (selected, _) = select_follow_candidates(results, 10);
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
    let (selected, _) = select_follow_candidates(results, 4);
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
    let (selected, _) = select_follow_candidates(results, 3);
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
    let (selected, _) = select_follow_candidates(results, 10);
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
    // Generic sign-in / login / auth URLs
    assert!(is_junk_url("https://example.com/signin"));
    assert!(is_junk_url("https://example.com/sign-in"));
    assert!(is_junk_url("https://example.com/login"));
    assert!(is_junk_url("https://example.com/log-in"));
    assert!(is_junk_url("https://example.com/auth"));
    assert!(is_junk_url("https://example.com/auth/continue"));
    // These should NOT be junk
    assert!(!is_junk_url("https://en.wikipedia.org/wiki/Entropy"));
    assert!(!is_junk_url("https://arxiv.org/abs/2112.06034"));
    assert!(!is_junk_url("https://example.com/page"));
    assert!(!is_junk_url("https://example.com/author"));
}

#[test]
fn test_is_junk_title_detection() {
    assert!(is_junk_title("Sign in to continue"));
    assert!(is_junk_title("Log in to continue reading"));
    assert!(is_junk_title("Please sign in to view this article"));
    assert!(is_junk_title("Sign in required"));
    // Legitimate article titles mentioning login must NOT be junk
    assert!(!is_junk_title("How to build a login system in Rust"));
    assert!(!is_junk_title("Understanding OAuth authentication flows"));
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
    let (selected, _) = select_follow_candidates(results, 10);
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
    let (selected, domains) = select_follow_candidates(vec![], 10);
    assert!(selected.is_empty(), "empty input must produce empty output");
    assert!(
        domains.is_empty(),
        "empty input must produce empty domain set"
    );

    let (selected, _) = select_follow_candidates(vec![], 0);
    assert!(
        selected.is_empty(),
        "empty input with max=0 must produce empty output"
    );
}

#[test]
fn test_classify_body_status_keeps_dense_prose() {
    // A dense/repetitive article with real paragraphs must be kept (Ok),
    // even if it carries low-entropy / boilerplate signals.
    let content = "The core thesis of this report is that the market will continue to grow. \
                       The market grows because demand grows, and demand grows because adoption \
                       grows. We repeat this point many times throughout the report to emphasize \
                       the importance of growth in the market. Share this article with your \
                       colleagues and listen to our podcast for more analysis. The market grows, \
                       demand grows, adoption grows, and the report repeats these themes over and \
                       over again to make the central argument unmistakably clear to every reader.";
    let fr = FollowResult {
        url: "https://example.com/a".into(),
        title: "t".into(),
        content: content.into(),
        error: String::new(),
        provenance: Provenance {
            source_url: "https://www.google.com/search?q=test".into(),
            method: ExtractionMethod::Search,
            agent: gthings_common::user_agent::gthings_agent(),
            accessed_at: Utc::now(),
            duration_ms: 100,
        },
        pagination: None,
        quality_flags: vec![],
    };
    let quality = compute_quality_with_flags(&fr.content, false, &fr.quality_flags);
    let status = classify_body_status(&fr, &quality);
    assert!(
        matches!(status, BodyStatus::Ok),
        "dense prose must be kept as Ok, got {:?} (quality {:?})",
        status,
        quality
    );
}

#[test]
fn test_classify_body_status_keeps_long_article_with_nav_tokens() {
    // A long article (15k chars) that happens to contain nav-menu tokens
    // must be kept as Ok, never dropped as ChromeOrEmpty.
    let mut content = String::new();
    while content.len() < 15_000 {
        content.push_str(
            "This is a real paragraph of article prose discussing the topic at length. \
                 The blog and pricing pages are mentioned in passing, but the bulk of this \
                 text is genuine content that a reader would want to consume. ",
        );
    }
    let fr = FollowResult {
        url: "https://example.com/article".into(),
        title: "t".into(),
        content: content.clone(),
        error: String::new(),
        provenance: Provenance {
            source_url: "https://www.google.com/search?q=test".into(),
            method: ExtractionMethod::Search,
            agent: gthings_common::user_agent::gthings_agent(),
            accessed_at: Utc::now(),
            duration_ms: 100,
        },
        pagination: None,
        quality_flags: vec![],
    };
    let quality = compute_quality_with_flags(&fr.content, false, &fr.quality_flags);
    let status = classify_body_status(&fr, &quality);
    assert!(
        matches!(status, BodyStatus::Ok),
        "long article with nav tokens must be kept as Ok, got {:?} (quality {:?})",
        status,
        quality
    );
}

#[test]
fn test_classify_body_status_drops_nav_only() {
    // A genuinely nav-only page (repeated menu tokens, no prose) must be
    // dropped as ChromeOrEmpty.
    let content = "About Us Contact Us Privacy Policy Terms of Service Careers Pricing Blog \
                       Sign In Log In FAQ Help Center Get Started About Us Contact Us Privacy \
                       Policy Terms of Service Careers Pricing Blog Sign In Log In FAQ Help Center";
    let fr = FollowResult {
        url: "https://example.com/nav".into(),
        title: "t".into(),
        content: content.into(),
        error: String::new(),
        provenance: Provenance {
            source_url: "https://www.google.com/search?q=test".into(),
            method: ExtractionMethod::Search,
            agent: gthings_common::user_agent::gthings_agent(),
            accessed_at: Utc::now(),
            duration_ms: 100,
        },
        pagination: None,
        quality_flags: vec![],
    };
    let quality = compute_quality_with_flags(&fr.content, false, &fr.quality_flags);
    let status = classify_body_status(&fr, &quality);
    assert!(
        matches!(status, BodyStatus::ChromeOrEmpty),
        "nav-only page must be dropped as ChromeOrEmpty, got {:?}",
        status
    );
}

#[test]
fn test_classify_body_status_drops_empty() {
    let fr = FollowResult {
        url: "https://example.com/empty".into(),
        title: "t".into(),
        content: String::new(),
        error: String::new(),
        provenance: Provenance {
            source_url: "https://www.google.com/search?q=test".into(),
            method: ExtractionMethod::Search,
            agent: gthings_common::user_agent::gthings_agent(),
            accessed_at: Utc::now(),
            duration_ms: 100,
        },
        pagination: None,
        quality_flags: vec![],
    };
    let quality = compute_quality_with_flags(&fr.content, false, &fr.quality_flags);
    let status = classify_body_status(&fr, &quality);
    assert!(
        matches!(status, BodyStatus::ChromeOrEmpty),
        "empty content must be dropped as ChromeOrEmpty, got {:?}",
        status
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
    let (selected, domains_selected) = select_follow_candidates(results, 3);

    // The returned domain set is computed in the same pass as selection
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
