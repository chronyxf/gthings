//! Budget-aware multi-engine search router.
//!
//! Holds the five concrete engine backends (Brave, Bing, Google, Brave API,
//! Tavily) plus a shared [`RouterState`] that tracks per-engine
//! cooldowns (after rate-limit/captcha blocks) and query budgets (minimum
//! interval between queries). [`SearchRouter::search_with_fallback`] tries
//! engines in the active [`EngineMode`] priority order, skipping any engine
//! that is cooling down or over budget, and falls back to the next engine on
//! failure.
//!
//! gthings runs as ONE long-lived process (or is embedded as a library), so
//! the shared [`PacingStore`] enforces minimum intervals and cooldowns across
//! the whole process. When a pacing directory is configured
//! (`GTHINGS_PACING_DIR`, falling back to `GTHINGS_REPUTATION_DIR`) the store
//! is loaded from disk on first access and persisted on every mutation, so
//! last-call timestamps and cooldowns survive a daemon restart. Without a
//! configured directory pacing is fully in-memory. Auto mode skips engines
//! whose interval has not elapsed (same as an over-budget engine); pin mode
//! politely waits out the remaining interval before dispatching.
//!
//! Implementation split into submodules:
//! - [`select`] — engine selection, pacing, cooldowns, fallback accumulation.
//! - [`dispatch`] — dispatch, fallback search, outcome recording, observer.
//! - [`mapping`] — result filtering, dedup, and classification.
//!
//! Unit tests live in `tests/`.

mod dispatch;
mod mapping;
mod select;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::Notify;

use crate::engine::api::brave::BraveApiBackend;
use crate::engine::api::tavily::TavilyBackend;
use crate::engine::pacing::{PacingStore, global_pacing};
use crate::engine::scrape::bing::BingBackend;
use crate::engine::scrape::brave::BraveBackend;
use crate::engine::scrape::google::GoogleBackend;
use crate::engine::{EngineMode, SearchEngine};

use self::select::{seed_cooldowns, unix_now_ms};

#[cfg(test)]
pub(crate) use self::dispatch::BoxedSearch;
pub use self::dispatch::DispatchOutcome;
pub(crate) use self::mapping::{
    dedup_by_base_key, is_empty_snippet, is_fragment_url, is_translate_wrapper_url,
    map_engine_results,
};

/// Cooldown applied after an engine reports a rate-limit.
pub(crate) const RATE_LIMITED_COOLDOWN: Duration = Duration::from_secs(5 * 60);

/// Cooldown applied after an engine reports a captcha/block page.
pub(crate) const CAPTCHA_COOLDOWN: Duration = Duration::from_secs(30 * 60);

/// Max time auto mode waits for any engine to become eligible (off cooldown /
/// over-budget interval) before giving up with [`SearchEngineError::Unavailable`].
///
/// Must stay comfortably under the 30s per-query orchestrator timeout — the
/// bounded wait plus the dispatch has to fit inside it. 10s of waiting plus
/// the 15s HTTP-client dispatch timeout leaves ~5s of headroom under 30s.
pub(crate) const MAX_AUTO_WAIT: Duration = Duration::from_secs(10);

/// Sleep granularity of the auto-mode wait loop: every pass re-reads the
/// persisted pacing state and re-checks eligibility.
pub(crate) const WAIT_POLL: Duration = Duration::from_millis(500);

/// Thread-safe router state shared across concurrent queries.
///
/// - `cooldowns`: engine → instant at which its block (rate-limit/captcha)
///   expires; the engine is skipped until that instant.
///
/// Minimum-interval pacing is *not* tracked here: it lives solely in the
/// [`PacingStore`] (last-call unix-ms timestamps), which is the single source
/// of truth for query budgets. Keeping one clock avoids the drift that a
/// second `Instant`-based budget map would introduce.
#[derive(Debug, Default)]
pub struct RouterState {
    /// Cooldown expiry instants, keyed by engine (rate-limit/captcha blocks).
    cooldowns: HashMap<SearchEngine, Instant>,
}

/// Multi-engine search router with fallback, cooldowns, and query budgets.
pub struct SearchRouter {
    /// CDP Brave backend; only present when a browser session was supplied.
    brave: Option<BraveBackend>,
    /// Plain-HTTP Bing backend (RSS endpoint; always available).
    bing: BingBackend,
    /// CDP Google backend; only present when a browser session was supplied.
    google: Option<GoogleBackend>,
    /// Paid Brave Search API backend (plain HTTP).
    brave_api: BraveApiBackend,
    /// Paid Tavily backend (plain HTTP).
    tavily: TavilyBackend,
    /// Hybrid engine routing mode (free/hybrid/api).
    mode: EngineMode,
    /// Shared cooldown/budget state, guarded for concurrent queries.
    state: Mutex<RouterState>,
    /// Persistent per-engine last-call timestamps, guarded for concurrent
    /// queries; enforces minimum intervals across CLI invocations.
    pacing: Arc<Mutex<PacingStore>>,
    /// Wakes the auto-mode wait loop when pacing/cooldown state changes, so it
    /// reacts immediately instead of busy-polling.
    notify: Notify,
    /// Test-only dispatch intercept: when set, every dispatch is served by
    /// this boxed search instead of the real engine backends, letting offline
    /// tests drive the full router/streaming path with canned outcomes.
    #[cfg(test)]
    fake_backend: Option<BoxedSearch>,
}

impl SearchRouter {
    /// Create a router with all five backends.
    ///
    /// Bing speaks plain HTTP (RSS endpoint) and is always constructed; the
    /// CDP-backed Brave and Google backends are constructed only
    /// when `session` is `Some`. Without a browser session those engines fail
    /// with [`SearchEngineError::Unavailable`] and are skipped in Auto mode.
    ///
    /// Pacing is **process-wide** and, when a directory is configured,
    /// **disk-persisted**: this router shares the single [`global_pacing`]
    /// store with every other router built in this process, so
    /// minimum-interval pacing and cooldowns survive both router rebuilds and
    /// daemon restarts (e.g. when the orchestrator rebuilds a router per
    /// harvest call). Cooldowns still in the future are seeded into the
    /// in-memory map; expired ones are dropped.
    pub fn new(session: Option<Arc<gthings_cdp::Session>>) -> Self {
        Self::with_pacing(session, global_pacing().clone())
    }

    /// Create a router sharing the given `pacing` store.
    ///
    /// Test-friendly constructor: lets tests inject an isolated
    /// [`PacingStore`] instead of the process-wide global, avoiding
    /// cross-test state pollution. Production code should use [`Self::new`],
    /// which shares the global store.
    ///
    /// Shared construction body: backends, pacing store, and routing mode
    /// (always resolved from `GTHINGS_ENGINE_MODE`, default `hybrid`).
    pub(crate) fn with_pacing(
        session: Option<Arc<gthings_cdp::Session>>,
        pacing: Arc<Mutex<PacingStore>>,
    ) -> Self {
        let state = RouterState {
            // Cooldowns still in the future are seeded into the in-memory
            // map; expired ones are dropped.
            cooldowns: seed_cooldowns(
                &pacing.lock().unwrap_or_else(|e| e.into_inner()),
                Instant::now(),
                unix_now_ms(),
            ),
        };
        Self {
            brave: session.clone().map(BraveBackend::new),
            bing: BingBackend::new(),
            google: session.map(GoogleBackend::new),
            brave_api: BraveApiBackend,
            tavily: TavilyBackend,
            // Hybrid routing mode (free|hybrid|api), always resolved from
            // `GTHINGS_ENGINE_MODE` (default hybrid) — a daemon-level concern,
            // never threaded in by the CLI.
            mode: EngineMode::from_env(),
            state: Mutex::new(state),
            // Shared last-call timestamps enforce minimum intervals for the
            // lifetime of this process (and across restarts when a pacing
            // directory is configured).
            pacing,
            notify: Notify::new(),
            #[cfg(test)]
            fake_backend: None,
        }
    }

    /// Test-only constructor: serve every dispatch from `fake` so tests can
    /// drive the full router/streaming path offline with canned outcomes.
    #[cfg(test)]
    pub(crate) fn with_fake_backend(pacing: Arc<Mutex<PacingStore>>, fake: BoxedSearch) -> Self {
        let mut router = Self::with_pacing(None, pacing);
        router.fake_backend = Some(fake);
        router
    }

    /// The hybrid engine routing mode this router dispatches under.
    pub(crate) fn mode(&self) -> EngineMode {
        self.mode
    }
}
