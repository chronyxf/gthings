//! Multi-engine search facade.
//!
//! Routes queries through the [`SearchRouter`](crate::engine::router::SearchRouter),
//! which tries engines in priority order — Google → Brave → Bing —
//! with per-engine cooldowns (after rate-limits/captchas), query budgets
//! (minimum interval between queries), and automatic fallback on failure.

use std::sync::Arc;
use std::time::Duration;

use gthings_cdp::{CdpError, Session, TabGuard};
use gthings_common::provenance::Provenance;
use serde::{Deserialize, Serialize};

use crate::engine::router::SearchRouter;
use crate::engine::{EngineChoice, EngineMode, SearchEngine, SearchEngineError, SearchOptions};
use crate::follow::TimedSearchOutcome;
use crate::stream::SearchEvent;
use crate::streaming::search_streaming;

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
    /// Engine that produced this result.
    ///
    /// Additive field (`#[serde(default)]`): older result envelopes without
    /// it deserialize to the default engine.
    #[serde(default = "default_engine")]
    pub engine: SearchEngine,
    /// Relevance score (0.0–1.0): backend-supplied when available, otherwise
    /// derived from the result position.
    #[serde(default)]
    pub score: f64,
    /// Publication date when the backend exposed one; `None` otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_date: Option<String>,
    /// Favicon URL when the backend exposed one; `None` otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub favicon: Option<String>,
    /// Hybrid engine routing mode used for the job that produced this result.
    #[serde(default = "default_mode")]
    pub mode: EngineMode,
}

/// Default engine for deserializing result envelopes that predate the
/// additive `engine` field.
fn default_engine() -> SearchEngine {
    SearchEngine::Brave
}

/// Default routing mode for deserializing result envelopes that predate the
/// additive `mode` field.
fn default_mode() -> EngineMode {
    EngineMode::Hybrid
}

/// Execute a search in multi-engine auto mode.
///
/// Builds a [`SearchRouter`] with the CDP-backed Brave and Google backends
/// enabled via `session` (Bing and the paid API backends speak plain HTTP and
/// are always available), then runs [`SearchRouter::search_with_fallback`] with
/// [`EngineChoice::Auto`]: engines are tried in priority order
/// (Google → Brave → Bing), skipping any that are cooling down
/// or over budget, and falling back to the next engine on failure. The first
/// successful engine's results win.
///
/// # Arguments
///
/// * `session` — The CDP session managing the browser connection (enables the
///   Brave and Google backends, which manage their own background tabs).
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
/// When `session` is `None`, only the plain-HTTP engines (Bing, plus the paid
/// Brave API / Tavily backends) are available; the browser-only engines
/// (Brave, Google) are skipped in [`EngineChoice::Auto`] mode, and pinning one
/// fails with
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
///
/// The routing mode (`free`/`hybrid`/`api`) is a daemon-level concern: the
/// router resolves it from `GTHINGS_ENGINE_MODE` (default `hybrid`), never
/// from a CLI flag.
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

    let router = Arc::new(SearchRouter::new(session.cloned()));
    search_with_router(router, query, count, choice).await
}

/// Run a search through a caller-provided shared router.
///
/// Used by the batch path so all queries in a batch share ONE router (and
/// thus one in-memory [`PacingStore`]), letting pacing coordinate across the
/// whole batch. The single-query path constructs its own router as before.
///
/// This is a projection over the streaming facade
/// ([`crate::streaming::search_streaming`]): the router's [`SearchEvent`]
/// stream is consumed and the [`SearchEvent::Result`] events are collected
/// into a plain `Vec`, with the first [`SearchEvent::Error`] surfaced as the
/// call's error. The router is taken by [`Arc`] so the underlying stream task
/// is `'static`; callers already hold the shared router as an `Arc` (see
/// [`crate::batch::BatchProcessor`]).
///
/// # Arguments
///
/// * `router` — The shared [`SearchRouter`] to dispatch through.
/// * `query` — The search query string.
/// * `count` — Maximum number of search results to return.
/// * `choice` — [`EngineChoice::Auto`] (priority-order fallback) or
///   [`EngineChoice::Pin`] (single engine attempt, no fallback).
pub async fn search_with_router(
    router: Arc<SearchRouter>,
    query: &str,
    count: usize,
    choice: EngineChoice,
) -> Result<Vec<SearchResult>, SearchEngineError> {
    let mut rx = search_streaming(
        router,
        query.to_string(),
        count,
        choice,
        &SearchOptions::default(),
    );
    let mut results = Vec::new();
    while let Some(event) = rx.recv().await {
        match event {
            SearchEvent::Result(result) => results.push(*result),
            SearchEvent::Error(e) => return Err(e),
            // JobStarted / EngineEvent / Done are progress metadata: the
            // collect facade drops them.
            _ => {}
        }
    }
    Ok(results)
}

/// Run a search through a shared router inside a temporary background tab
/// with a per-query timeout (batch path).
///
/// Opens a fresh background tab, runs [`search_with_router`] inside it with
/// the shared router — so all queries in a batch dispatch through the same
/// instance and share a single in-memory [`PacingStore`], letting pacing
/// coordinate across the batch — and closes the tab on every exit path via a
/// [`TabGuard`]. Errors from tab creation bubble up; tab-close failures are
/// logged but not propagated. A hit on `timeout` is returned as
/// [`TimedSearchOutcome::Timeout`].
pub(crate) async fn search_with_router_tab(
    router: Arc<SearchRouter>,
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
