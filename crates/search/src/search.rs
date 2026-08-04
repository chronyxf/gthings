//! Multi-engine search facade.
//!
//! Routes queries through the [`SearchRouter`](crate::engine::router::SearchRouter),
//! which tries engines in priority order — Brave → Bing → Google —
//! with per-engine cooldowns (after rate-limits/captchas), query budgets
//! (minimum interval between queries), and automatic fallback on failure.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gthings_cdp::{CdpError, Session, TabGuard};
use gthings_common::provenance::Provenance;
use serde::{Deserialize, Serialize};

use crate::engine::router::{SearchRouter, map_engine_results};
use crate::engine::{EngineChoice, SearchEngineError};
use crate::follow::TimedSearchOutcome;

/// A single search result with provenance metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub position: usize,
    /// How and when this result was obtained.
    #[serde(default, skip)]
    pub provenance: Provenance,
    /// Domain authority score (0.0–1.0) for the result URL's host.
    #[serde(default)]
    pub domain_authority: f64,
    /// Coarse source classification derived from the result URL
    /// (e.g. "github", "paper", "pdf", or "web").
    #[serde(default)]
    pub source_type: String,
}

/// Execute a search in multi-engine auto mode.
///
/// Builds a [`SearchRouter`] with browser backends (Bing, Google) enabled via
/// `session`, then runs [`SearchRouter::search_with_fallback`] with
/// [`EngineChoice::Auto`]: engines are tried in priority order
/// (Brave → Bing → Google), skipping any that are cooling down
/// or over budget, and falling back to the next engine on failure. The first
/// successful engine's results win.
///
/// # Arguments
///
/// * `session` — The CDP session managing the browser connection (enables the
///   Bing and Google backends, which manage their own background tabs).
/// * `query` — The search query string.
/// * `count` — Maximum number of search results to return.
pub async fn search(
    session: &Arc<Session>,
    query: &str,
    count: usize,
) -> Result<Vec<SearchResult>, SearchEngineError> {
    search_with_engine(Some(session), query, count, EngineChoice::Auto).await
}

/// Execute a search with an explicit engine choice.
///
/// When `session` is `None`, only the plain-HTTP engines (Brave)
/// are available; browser-only engines (Bing, Google) are skipped in
/// [`EngineChoice::Auto`] mode, and pinning one fails with
/// [`SearchEngineError::Unavailable`].
///
/// # Arguments
///
/// * `session` — Optional CDP session; `None` builds a router with no browser
///   engines (HTTP only).
/// * `query` — The search query string.
/// * `count` — Maximum number of search results to return.
/// * `choice` — [`EngineChoice::Auto`] (priority-order fallback) or
///   [`EngineChoice::Pin`] (single engine attempt, no fallback).
pub async fn search_with_engine(
    session: Option<&Arc<Session>>,
    query: &str,
    count: usize,
    choice: EngineChoice,
) -> Result<Vec<SearchResult>, SearchEngineError> {
    // Pinning a browser engine without a session can never succeed — reject up front.
    if let EngineChoice::Pin(engine) = choice {
        if session.is_none() && engine.requires_browser() {
            return Err(SearchEngineError::Unavailable {
                engine,
                detail: "browser session required".to_string(),
            });
        }
    }

    let router = SearchRouter::new(session.cloned());
    search_with_router(&router, query, count, choice).await
}

/// Run a search through a caller-provided router.
///
/// Used by the batch path so all queries in a batch share ONE router (and
/// thus one in-memory [`PacingStore`]), letting pacing coordinate across the
/// whole batch. The single-query path constructs its own router as before.
///
/// # Arguments
///
/// * `router` — The shared [`SearchRouter`] to dispatch through.
/// * `query` — The search query string.
/// * `count` — Maximum number of search results to return.
/// * `choice` — [`EngineChoice::Auto`] (priority-order fallback) or
///   [`EngineChoice::Pin`] (single engine attempt, no fallback).
pub async fn search_with_router(
    router: &SearchRouter,
    query: &str,
    count: usize,
    choice: EngineChoice,
) -> Result<Vec<SearchResult>, SearchEngineError> {
    let start = Instant::now();
    let results = router.search_with_fallback(query, count, choice).await?;
    let duration_ms = start.elapsed().as_millis() as u64;

    Ok(map_engine_results(results, query, duration_ms))
}

/// Run a search through a shared router inside a temporary tab with a
/// per-query timeout (batch path).
///
/// Mirrors the single-query timed-tab search but threads the shared router
/// so all queries in a batch dispatch through the same instance — and thus
/// share a single in-memory [`PacingStore`], letting pacing coordinate across
/// the batch. Errors from tab creation bubble up; tab-close failures are
/// logged but not propagated. Timeout is returned as
/// [`TimedSearchOutcome::Timeout`].
pub(crate) async fn search_with_router_tab(
    router: &SearchRouter,
    session: &Arc<Session>,
    query: &str,
    count: usize,
    timeout: Duration,
) -> Result<TimedSearchOutcome, CdpError> {
    let tab = session.create_background_tab().await?;
    // RAII guard closes the tab on ALL exit paths (success, error, cancellation).
    let _guard = TabGuard::new(session, tab);
    let result = tokio::time::timeout(
        timeout,
        search_with_router(router, query, count, EngineChoice::Auto),
    )
    .await;
    match result {
        Ok(Ok(results)) => Ok(TimedSearchOutcome::Success(results)),
        Ok(Err(e)) => Ok(TimedSearchOutcome::Error(CdpError::CdpCallFailed {
            method: "search".into(),
            detail: e.to_string(),
        })),
        Err(_) => Ok(TimedSearchOutcome::Timeout),
    }
}
