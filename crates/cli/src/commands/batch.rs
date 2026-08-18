//! Batch search (dispatched via `search --strategy parallel`).

use gthings_common::taxonomy::ErrorCode;
use gthings_search::{BatchProcessor, BatchSearchConfig};

use crate::EngineFlag;
use crate::util::{UniversalFlags, emit_error, emit_success, with_session};

/// Batch: detect → connect → batch → disconnect → output.
///
/// BatchProcessor::search creates and closes tabs internally, so we only
/// manage the session lifecycle at this level. The batch strategy always
/// auto-routes engines under the router's env-resolved routing mode
/// (`GTHINGS_ENGINE_MODE`); pinned engines (brave/bing/google) are ignored
/// with a warning.
pub(crate) async fn cmd_batch(
    flags: &UniversalFlags,
    queries: Vec<String>,
    count: usize,
    extract_results: bool,
    max_chars: usize,
    engine: EngineFlag,
) -> i32 {
    // The batch strategy always auto-routes engines; pinned engines are
    // ignored (a warning is emitted).
    if engine.to_search_engine().is_some() {
        eprintln!(
            "gthings: --engine brave|bing|google is ignored for the parallel (batch) strategy; batch always auto-routes under GTHINGS_ENGINE_MODE"
        );
    }
    let config = BatchSearchConfig {
        follow_results: extract_results,
        follow_max_chars: max_chars,
        reputation: None,
    };

    with_session(flags, |session| async move {
        let all_results = match BatchProcessor::search(session, &queries, count, config).await {
            Ok(r) => r,
            Err(e) => {
                emit_error(
                    flags,
                    ErrorCode::EngineFailed,
                    &e.to_string(),
                    "Retry with fewer queries or longer timeout",
                );
                return 1;
            }
        };

        // Each query yields its own result or error entry (per-query isolation).
        let serializable: Vec<serde_json::Value> = all_results
            .into_iter()
            .map(|r| match r {
                Ok(results) => serde_json::json!({ "ok": results }),
                Err(e) => serde_json::json!({ "error": e.to_string() }),
            })
            .collect();

        emit_success(flags, serde_json::json!({"results": serializable}));
        0
    })
    .await
}
