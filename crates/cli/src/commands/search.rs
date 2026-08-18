//! `gthings search` — multi-engine search via CDP.
//!
//! Uses the multi-engine facade (`gthings_search::search_with_engine`):
//! backends own their background tabs, so no isolated-tab plumbing is needed.
//! Plain-HTTP engines (Brave, Bing) never require a browser;
//! Google does, so a CDP session is only opened when one is needed.

use std::sync::Arc;

use gthings_common::taxonomy::ErrorCode;
use gthings_search::search_with_engine;

use crate::EngineFlag;
use crate::SearchArgs;
use crate::SearchStrategy;
use crate::util::{UniversalFlags, connect, emit_error, emit_success};

/// Search: owns the strategy split (simple / parallel / harvest) and the
/// empty-query validation, then dispatches to the strategy handler.
pub(crate) async fn cmd_search(flags: &UniversalFlags, args: SearchArgs) -> i32 {
    match args.strategy {
        SearchStrategy::Simple => {
            let term = args.queries.first().map(String::as_str).unwrap_or("");
            if term.trim().is_empty() {
                emit_error(
                    flags,
                    ErrorCode::InvalidInput,
                    "Search query cannot be empty",
                    "Provide a search term",
                );
                return 1;
            }
            cmd_search_simple(flags, term, args.count, args.engine).await
        }
        SearchStrategy::Parallel => {
            crate::commands::cmd_batch(
                flags,
                args.queries,
                args.count,
                args.extract_results,
                args.max_chars,
                args.engine,
            )
            .await
        }
        SearchStrategy::Harvest => {
            crate::commands::cmd_harvest(
                flags,
                args.queries,
                args.rank,
                args.follow_top,
                args.max_chars,
                args.warn_tabs,
                args.engine,
            )
            .await
        }
    }
}

/// Simple strategy: detect → connect (only when a browser engine needs it) →
/// search (engine facade) → disconnect (only when connected) → output.
async fn cmd_search_simple(
    flags: &UniversalFlags,
    term: &str,
    count: usize,
    engine: EngineFlag,
) -> i32 {
    let term_owned = term.to_string();
    let choice = engine.to_choice();

    // Open a CDP session only when a browser engine (Google) is involved.
    // HTTP engines (Brave, Bing) work with `None`; Auto degrades to the HTTP
    // engines when no browser is available instead of failing.
    let session: Option<Arc<gthings_cdp::Session>> = match engine {
        EngineFlag::Brave | EngineFlag::Bing => None,
        EngineFlag::Google | EngineFlag::Auto => match connect(flags).await {
            Ok(s) => Some(Arc::new(s)),
            Err(c) if engine == EngineFlag::Google => return c,
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
                drop(Arc::clone(s));
            }
            emit_error(
                flags,
                ErrorCode::EngineFailed,
                &e.to_string(),
                "Retry with different arguments",
            );
            return 1;
        }
    };

    if let Some(s) = &session {
        drop(Arc::clone(s));
    }

    let data = serde_json::json!({
        "results": result,
        "query": term_owned,
    });
    emit_success(flags, data);
    0
}
