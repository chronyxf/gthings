//! Harvest pipeline (dispatched via `search --strategy harvest`).
//!
//! Full research pipeline: search → dedup → rank → select → follow → quality score.

use std::sync::Arc;

use gthings_common::domain_reputation::DomainReputation;
use gthings_common::pagination::ExtractParams;
use gthings_common::taxonomy::ErrorCode;
use gthings_search::harvest::{BatchHarvestRequest, RankStrategy, harvest};

use crate::EngineFlag;
use crate::args::RankFlag;
use crate::util::{UniversalFlags, emit_error, emit_success, with_session};

/// Default reputation cache directory name under the OS temp dir.
const REPUTATION_DIR_NAME: &str = "gthings-reputation";

/// Harvest: detect → connect → harvest → disconnect → output.
pub(crate) async fn cmd_harvest(
    flags: &UniversalFlags,
    queries: Vec<String>,
    rank: RankFlag,
    follow_top: usize,
    max_chars: usize,
    warn_tabs: usize,
    engine: EngineFlag,
) -> i32 {
    let rank_strategy = match rank {
        RankFlag::Serp => RankStrategy::SerpOrder,
        RankFlag::Authority => RankStrategy::DomainAuthority,
        RankFlag::Snippet => RankStrategy::SnippetLength,
        RankFlag::Composite => RankStrategy::Composite,
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

    // Reputation cache location/TTL from env (`GTHINGS_REPUTATION_DIR`,
    // `GTHINGS_REPUTATION_TTL_SECS`); defaults are the OS temp dir + 24h.
    let cfg = gthings_common::config::Config::load();
    let reputation_dir = cfg
        .reputation_dir
        .clone()
        .unwrap_or_else(|| std::env::temp_dir().join(REPUTATION_DIR_NAME));
    let reputation = Arc::new(DomainReputation::new(
        reputation_dir,
        cfg.reputation_ttl_secs, // default 86400
    ));

    let req = build_harvest_request(
        queries,
        rank_strategy,
        follow_top,
        max_chars,
        Some(reputation),
        engine,
    );

    with_session(flags, |session| async move {
        let (results, summary) = match harvest(session, req).await {
            Ok(r) => r,
            Err(e) => {
                emit_error(
                    flags,
                    ErrorCode::EngineFailed,
                    &e.to_string(),
                    "Check browser connection and network",
                );
                return 1;
            }
        };

        let value = serde_json::json!({
            "results": results,
            "summary": summary,
        });
        emit_success(flags, value);
        0
    })
    .await
}

/// Build the [`BatchHarvestRequest`] for the harvest pipeline, threading the
/// CLI's pinned `--engine brave|bing|google` value into `engine`; the search
/// phase's routing mode is resolved by the router from `GTHINGS_ENGINE_MODE`.
fn build_harvest_request(
    queries: Vec<String>,
    rank_by: RankStrategy,
    follow_top_n: usize,
    max_chars: usize,
    reputation: Option<Arc<DomainReputation>>,
    engine: EngineFlag,
) -> BatchHarvestRequest {
    BatchHarvestRequest {
        queries,
        rank_by,
        follow_top_n,
        extract_params: ExtractParams {
            offset: 0,
            max_chars,
        },
        reputation,
        engine: engine.to_search_engine(),
    }
}
