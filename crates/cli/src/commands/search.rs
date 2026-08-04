//! `gthings search` — multi-engine search via CDP.
//!
//! Uses the multi-engine facade (`gthings_search::search_with_engine`):
//! backends own their background tabs, so no isolated-tab plumbing is needed.
//! Plain-HTTP engines (Brave, Bing) never require a browser;
//! Google does, so a CDP session is only opened when one is needed.

use std::sync::Arc;

use gthings_search::search_with_engine;

use crate::commands::{UniversalFlags, connect, disconnect_session, emit_output};
use crate::EngineFlag;

/// Search: detect → connect (only when a browser engine needs it) → search
/// (engine facade) → disconnect (only when connected) → output.
pub(crate) async fn cmd_search(
    flags: &UniversalFlags,
    term: &str,
    count: usize,
    engine: EngineFlag,
) -> i32 {
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

    let query_for_output = term.to_string();
    let term_owned = term.to_string();
    let choice = engine.to_choice();

    // Open a CDP session only when a browser engine (Google) is involved.
    // HTTP engines (Brave, Bing) work with `None`; Auto degrades
    // to the HTTP engines when no browser is available instead of failing.
    let session: Option<Arc<gthings_cdp::Session>> = match engine {
        EngineFlag::Brave | EngineFlag::Bing => None,
        EngineFlag::Google => match connect(flags).await {
            Ok(s) => Some(Arc::new(s)),
            Err(c) => return c,
        },
        EngineFlag::Auto => match connect(flags).await {
            Ok(s) => Some(Arc::new(s)),
            Err(c) => {
                tracing::warn!(
                    "Browser unavailable (connect code {c}); Google disabled, falling back to HTTP engines (brave, bing)"
                );
                None
            }
        },
    };

    let result = match search_with_engine(session.as_ref(), &term_owned, count, choice).await {
        Ok(r) => r,
        Err(e) => {
            if let Some(s) = &session {
                disconnect_session(Arc::clone(s)).await;
            }
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

    if let Some(s) = &session {
        disconnect_session(Arc::clone(s)).await;
    }

    let data = serde_json::json!({
        "results": result,
        "query": query_for_output,
    });
    emit_output(
        Some(data),
        None,
        flags.resolved_output(),
        flags.query.as_deref(),
    );
    0
}
