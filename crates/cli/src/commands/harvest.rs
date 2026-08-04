//! Harvest pipeline (dispatched via `search --strategy harvest`).
//!
//! Full research pipeline: search → dedup → rank → select → follow → quality score.

use std::sync::Arc;

use gthings_common::domain_reputation::DomainReputation;
use gthings_common::pagination::ExtractParams;
use gthings_search::harvest::{BatchHarvestRequest, DedupStrategy, RankStrategy, harvest};

use crate::EngineFlag;
use crate::commands::{UniversalFlags, emit_output, with_session};

/// Harvest: detect → connect → harvest → disconnect → output.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn cmd_harvest(
    flags: &UniversalFlags,
    queries: Vec<String>,
    dedup: String,
    rank: String,
    follow_top: usize,
    max_chars: usize,
    warn_tabs: usize,
    engine: EngineFlag,
) -> i32 {
    let dedup_strategy = if dedup.as_str() == "url" {
        DedupStrategy::UrlOnly
    } else {
        emit_output(
            None,
            Some((
                "INVALID_DEDUP",
                &format!("Unknown dedup strategy: {dedup}"),
                "Use --dedup=url",
            )),
            flags.resolved_output(),
            flags.query.as_deref(),
        );
        return 1;
    };

    let rank_strategy = match rank.as_str() {
        "serp" => RankStrategy::SerpOrder,
        "authority" => RankStrategy::DomainAuthority,
        "snippet" => RankStrategy::SnippetLength,
        "composite" => RankStrategy::Composite,
        _ => {
            emit_output(
                None,
                Some((
                    "INVALID_RANK",
                    &format!("Unknown rank strategy: {rank}"),
                    "Use --rank=serp|authority|snippet|composite",
                )),
                flags.resolved_output(),
                flags.query.as_deref(),
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
        engine: match engine {
            EngineFlag::Auto => None,
            other => Some(other.to_search_engine()),
        },
    };

    with_session(flags, |session| async move {
        let (results, summary) = match harvest(session, req).await {
            Ok(r) => r,
            Err(e) => {
                emit_output(
                    None,
                    Some((
                        "HARVEST_FAILED",
                        &e.to_string(),
                        "Check browser connection and network",
                    )),
                    flags.resolved_output(),
                    flags.query.as_deref(),
                );
                return 1;
            }
        };

        let value = serde_json::json!({
            "results": results,
            "summary": summary,
        });
        emit_output(
            Some(value),
            None,
            flags.resolved_output(),
            flags.query.as_deref(),
        );
        0
    })
    .await
}
