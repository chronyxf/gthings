//! `gthings search` — Google search via CDP.
//!
//! Uses isolated background tab pattern: each search creates its own tab,
//! preventing cross-process blocking when concurrent searches run.

use gthings_search::search;

use crate::commands::{UniversalFlags, connect, emit_output};

/// Search: detect → connect → isolated search → disconnect → output.
pub(crate) async fn cmd_search(flags: &UniversalFlags, term: &str, count: usize) -> i32 {
    if term.trim().is_empty() {
        emit_output(
            None,
            Some((
                "EMPTY_QUERY",
                "Search term cannot be empty",
                "Provide at least one non-empty search term",
            )),
            flags.resolved_output(),
            flags.query.as_deref(),
        );
        return 1;
    }
    let session = match connect(flags).await {
        Ok(s) => s,
        Err(c) => return c,
    };

    let term = term.to_string();
    let query_for_output = term.clone();
    let result = match session
        .with_isolated_tab(|session, tab| {
            Box::pin(async move { search(session, tab, &term, count).await })
        })
        .await
    {
        Ok(r) => r,
        Err(e) => {
            let _ = session.disconnect().await;
            emit_output(
                None,
                Some((
                    "SEARCH_FAILED",
                    &e.to_string(),
                    "Retry with different arguments",
                )),
                flags.resolved_output(),
                flags.query.as_deref(),
            );
            return 1;
        }
    };

    if let Err(e) = session.disconnect().await {
        tracing::warn!("disconnect failed: {e}");
    }

    let data = serde_json::json!({
        "results": result,
        "query": query_for_output,
        "body_status": "snippet_only",
    });
    emit_output(
        Some(data),
        None,
        flags.resolved_output(),
        flags.query.as_deref(),
    );
    0
}
