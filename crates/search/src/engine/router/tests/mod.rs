//! Router unit tests, split per implementation submodule.
//!
//! Shared fixtures ([`cold_state`], [`cold_pacing`], [`engine_result`]) live
//! here and are imported by the per-submodule test files below.

use std::collections::HashMap;

use crate::engine::pacing::PacingStore;
use crate::engine::router::RouterState;
use crate::engine::{EngineSearchResult, SearchEngine};

mod dispatch;
mod mapping;
mod select;

/// Cold router state: no cooldowns, no budget stamps.
fn cold_state() -> RouterState {
    RouterState {
        cooldowns: HashMap::new(),
    }
}

/// Empty pacing store: no last-call timestamps, no cooldowns.
fn cold_pacing() -> PacingStore {
    PacingStore::new()
}

fn engine_result(title: &str, url: &str, snippet: &str) -> EngineSearchResult {
    EngineSearchResult {
        title: title.to_string(),
        url: url.to_string(),
        snippet: snippet.to_string(),
        position: 0,
        engine: SearchEngine::Brave,
        score: 0.0,
        published_date: None,
        favicon: None,
    }
}
