//! Batch search (dispatched via `search --strategy parallel`).

use gthings_search::{BatchProcessor, BatchSearchConfig};

use crate::commands::{UniversalFlags, emit_output, with_session};
use crate::EngineFlag;

/// Batch: detect → connect → batch → disconnect → output.
///
/// BatchProcessor::search creates and closes tabs internally, so we only
/// manage the session lifecycle at this level. The batch strategy always
/// auto-routes engines; `--engine` is accepted for CLI consistency but
/// ignored (a warning is emitted when a non-auto engine is requested).
pub(crate) async fn cmd_batch(
    flags: &UniversalFlags,
    queries: Vec<String>,
    count: usize,
    extract_results: bool,
    max_chars: usize,
    engine: EngineFlag,
) -> i32 {
    if !matches!(engine, EngineFlag::Auto) {
        eprintln!("gthings: --engine is ignored for the parallel (batch) strategy; batch always auto-routes");
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
                emit_output(
                    None,
                    Some((
                        "BATCH_FAILED",
                        &e.to_string(),
                        "Retry with fewer queries or longer timeout",
                    )),
                    flags.resolved_output(),
                    flags.query.as_deref(),
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

        emit_output(
            Some(serde_json::json!({"results": serializable})),
            None,
            flags.resolved_output(),
            flags.query.as_deref(),
        );
        0
    })
    .await
}
