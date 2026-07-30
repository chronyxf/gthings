//! Batch search (dispatched via `search --strategy parallel`).

use gthings_search::{BatchProcessor, BatchSearchConfig};

use crate::commands::{UniversalFlags, emit_output, with_session};

/// Batch: detect → connect → batch → disconnect → output.
///
/// BatchProcessor::search creates and closes tabs internally, so we only
/// manage the session lifecycle at this level.
pub(crate) async fn cmd_batch(
    flags: &UniversalFlags,
    queries: Vec<String>,
    count: usize,
    extract_results: bool,
    max_chars: usize,
) -> i32 {
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

        emit_output(
            Some(serde_json::json!({"results": all_results})),
            None,
            flags.resolved_output(),
            flags.query.as_deref(),
        );
        0
    })
    .await
}
