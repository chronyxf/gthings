//! Batch search (dispatched via `search --strategy parallel`).

use std::sync::Arc;

use gthings_search::BatchProcessor;

use crate::commands::{UniversalFlags, connect, emit_output};

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
    let session = match connect(flags).await {
        Ok(s) => s,
        Err(c) => return c,
    };

    let arc_session = Arc::new(session);

    let all_results = match BatchProcessor::search(
        Arc::clone(&arc_session),
        &queries,
        count,
        extract_results,
        max_chars,
        None,
    )
    .await
    {
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

    // Clean disconnect when possible (unique reference).
    if let Ok(s) = Arc::try_unwrap(arc_session) {
        if let Err(e) = s.disconnect().await {
            tracing::warn!("disconnect failed: {e}");
        }
    }

    emit_output(
        Some(serde_json::json!(all_results)),
        None,
        flags.resolved_output(),
        flags.query.as_deref(),
    );
    0
}
