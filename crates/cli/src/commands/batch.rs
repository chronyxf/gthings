//! `gthings batch` — batch search multiple queries via CDP.

use std::sync::Arc;

use crate::commands::{connect, print_error};
use gthings_search::BatchProcessor;

/// Batch: detect → connect → batch → disconnect.
///
/// BatchProcessor::search creates and closes tabs internally, so we only
/// manage the session lifecycle at this level.
pub(crate) async fn cmd_batch(
    queries: Vec<String>,
    count: usize,
    do_follow: bool,
    max_chars: usize,
    json: bool,
) -> i32 {
    let session = match connect().await {
        Ok(s) => s,
        Err(c) => return c,
    };

    let arc_session = Arc::new(session);

    let all_results = match BatchProcessor::search(
        Arc::clone(&arc_session),
        &queries,
        count,
        do_follow,
        max_chars,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            print_error(
                "BATCH_FAILED",
                &e.to_string(),
                "Retry with fewer queries or longer timeout",
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

    if json {
        let output = serde_json::to_string(&all_results).unwrap_or_else(|e| {
            tracing::error!("serialize output failed: {e}");
            String::new()
        });
        println!("{}", output);
    } else {
        for (i, results) in all_results.iter().enumerate() {
            println!("Query #{}:", i + 1);
            for r in results {
                println!("  #{} {} — {}", r.position, r.title, r.url);
                if !r.snippet.is_empty() {
                    println!("    {}", r.snippet);
                }
            }
        }
    }
    0
}
