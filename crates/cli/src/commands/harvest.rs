//! `gthings harvest` — full research pipeline: search → dedup → rank → follow.

use std::sync::Arc;

use gthings_common::domain_reputation::DomainReputation;
use gthings_common::pagination::ExtractParams;
use gthings_search::harvest::{
    BatchHarvestRequest, DedupStrategy, HarvestWarning, RankStrategy, harvest,
};

use crate::commands::{connect, print_error};

/// Harvest: detect → connect → harvest → disconnect → output.
pub(crate) async fn cmd_harvest(
    queries: Vec<String>,
    dedup: String,
    rank: String,
    follow_top: usize,
    max_chars: usize,
    json: bool,
    warn_tabs: usize,
) -> i32 {
    let session = match connect().await {
        Ok(s) => s,
        Err(c) => return c,
    };

    let dedup_strategy = match dedup.as_str() {
        "url" => DedupStrategy::UrlOnly,
        _ => {
            print_error(
                "INVALID_DEDUP",
                &format!("Unknown dedup strategy: {dedup}"),
                "Use --dedup=url",
            );
            return 1;
        }
    };

    let rank_strategy = match rank.as_str() {
        "serp" => RankStrategy::SerpOrder,
        "authority" => RankStrategy::DomainAuthority,
        "snippet" => RankStrategy::SnippetLength,
        "composite" => RankStrategy::Composite,
        _ => {
            print_error(
                "INVALID_RANK",
                &format!("Unknown rank strategy: {rank}"),
                "Use --rank=serp|authority|snippet|composite",
            );
            return 1;
        }
    };

    let total_tabs = queries.len() + follow_top;
    if total_tabs > warn_tabs {
        tracing::warn!(
            "harvest will spawn up to {total_tabs} tabs ({}+{}); \
             consider lowering --follow-top or increasing --warn-tabs",
            queries.len(),
            follow_top,
        );
    }

    let reputation = Arc::new(DomainReputation::new(
        std::env::temp_dir().join("gthings-reputation"),
        86400, // 24-hour TTL
    ));

    let req = BatchHarvestRequest {
        queries,
        dedup: dedup_strategy,
        rank_by: rank_strategy,
        follow_top_n: follow_top,
        extract_params: ExtractParams {
            offset: 0,
            max_chars,
        },
        reputation: Some(reputation),
    };

    let arc_session = Arc::new(session);

    let (results, summary) = match harvest(Arc::clone(&arc_session), req).await {
        Ok(r) => r,
        Err(e) => {
            print_error(
                "HARVEST_FAILED",
                &e.to_string(),
                "Check browser connection and network",
            );
            if let Ok(s) = Arc::try_unwrap(arc_session) {
                if let Err(e) = s.disconnect().await {
                    tracing::warn!("disconnect failed: {e}");
                }
            }
            return 1;
        }
    };

    if let Ok(s) = Arc::try_unwrap(arc_session) {
        if let Err(e) = s.disconnect().await {
            tracing::warn!("disconnect failed: {e}");
        }
    }

    if json {
        let output = serde_json::to_string_pretty(&serde_json::json!({
            "results": results,
            "summary": summary,
        }))
        .unwrap_or_else(|e| {
            tracing::error!("serialize output failed: {e}");
            String::new()
        });
        println!("{output}");
    } else {
        // Print summary
        println!("=== Harvest Summary ===");
        println!(
            "Queries: {} | Results: {} | Unique sources: {}",
            summary.total_queries, summary.total_results, summary.unique_sources_followed
        );
        for (q, cov) in &summary.coverage_by_query {
            println!(
                "  [{q}] total={} ok={} failed={}",
                cov.total_hits, cov.followed_ok, cov.followed_failed
            );
        }
        for w in &summary.warnings {
            match w {
                HarvestWarning::FollowBudgetCollapsedToOneSite => {
                    println!("  [!] Warning: Follow budget collapsed to one site");
                }
                HarvestWarning::NoBodyForQuery(q) => {
                    println!("  [!] Warning: No body content for query '{q}'");
                }
                HarvestWarning::AllSnippetOnly => {
                    println!("  [!] Warning: All results are snippet-only");
                }
            }
        }
        println!();

        // Print each result
        for (i, r) in results.iter().enumerate() {
            println!(
                "#{} {} — {}  [{:.1}]",
                i + 1,
                r.search_result.title,
                r.search_result.url,
                r.search_result.domain_authority
            );
            if !r.search_result.snippet.is_empty() {
                println!("  {}", r.search_result.snippet);
            }
            if let Some(ref content) = r.followed_content {
                let preview: String = content.chars().take(200).collect();
                println!("  Content: {preview}...");
            }
            if let Some(ref q) = r.quality {
                println!(
                    "  Quality: {:.2}/1.0 — {}",
                    q.score,
                    if q.is_ok { "ok" } else { "low" }
                );
            }
        }
    }

    0
}
