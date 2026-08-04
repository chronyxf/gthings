//! Budget-aware multi-engine search router.
//!
//! Holds the three concrete engine backends (Brave, Bing, Google)
//! plus a shared [`RouterState`] that tracks per-engine cooldowns (after
//! rate-limit/captcha blocks) and query budgets (minimum interval between
//! queries). [`SearchRouter::search_with_fallback`] tries engines in
//! [`SearchEngine::PRIORITY`] order, skipping any engine that is cooling down
//! or over budget, and falls back to the next engine on failure.
//!
//! gthings runs as ONE long-lived process (or is embedded as a library), so
//! engine pacing is fully in-memory: the [`PacingStore`] holds last-call
//! timestamps and cooldowns for the lifetime of the process, with no disk
//! persistence. Auto mode skips engines whose interval has not elapsed (same
//! as an over-budget engine); pin mode politely waits out the remaining
//! interval before dispatching.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gthings_common::provenance::{ExtractionMethod, Provenance};
use gthings_common::GTHINGS_AGENT;
use tokio::sync::Notify;

use crate::engine::bing::BingBackend;
use crate::engine::brave::BraveBackend;
use crate::engine::google::GoogleBackend;
use crate::engine::html::collapse_whitespace;
use crate::engine::technique;
use crate::engine::pacing::{global_pacing, PacingStore};
use crate::engine::{
    EngineChoice, EngineSearchResult, SearchEngine, SearchEngineBackend, SearchEngineError,
};
use crate::SearchResult;

/// Minimum interval between Brave queries.
///
/// Benchmarked against the live server: it throttles ~1 query per 35-42s
/// (429s under sustained load), so a 30s interval sat only ~5s short of the
/// measured server limit. 60s gives a comfortable margin while still keeping
/// the token bucket from hammering the endpoint.
const BRAVE_MIN_INTERVAL: Duration = Duration::from_secs(60);

/// Minimum interval between Bing queries.
const BING_MIN_INTERVAL: Duration = Duration::from_secs(1);

/// Minimum interval between Google queries.
///
/// Google is precious — one public IP — so it is throttled hardest.
const GOOGLE_MIN_INTERVAL: Duration = Duration::from_secs(6);

/// Cooldown applied after an engine reports a rate-limit.
const RATE_LIMITED_COOLDOWN: Duration = Duration::from_secs(5 * 60);

/// Cooldown applied after an engine reports a captcha/block page.
const CAPTCHA_COOLDOWN: Duration = Duration::from_secs(30 * 60);

/// Max time auto mode waits for any engine to become eligible (off cooldown /
/// over-budget interval) before giving up with [`SearchEngineError::Unavailable`].
///
/// Must stay comfortably under the 30s per-query orchestrator timeout — the
/// bounded wait plus the dispatch has to fit inside it. 10s of waiting plus
/// the 15s HTTP-client dispatch timeout leaves ~5s of headroom under 30s.
const MAX_AUTO_WAIT: Duration = Duration::from_secs(10);

/// Sleep granularity of the auto-mode wait loop: every pass re-reads the
/// persisted pacing state and re-checks eligibility.
const WAIT_POLL: Duration = Duration::from_millis(500);/// Thread-safe router state shared across concurrent queries.
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

/// Minimum interval between queries for `engine` (the token-bucket refill).
fn min_interval(engine: SearchEngine) -> Duration {
    match engine {
        SearchEngine::Brave => BRAVE_MIN_INTERVAL,
        SearchEngine::Bing => BING_MIN_INTERVAL,
        SearchEngine::Google => GOOGLE_MIN_INTERVAL,
        // DuckDuckGo is never dispatched (removed); its interval is unused.
        SearchEngine::DuckDuckGo => Duration::ZERO,
    }
}

/// Whether `engine` may be dispatched right now: not cooling down.
///
/// Minimum-interval pacing is enforced separately via [`pacing_ready`] against
/// the [`PacingStore`] (the single source of truth for query budgets).
fn engine_ready(state: &RouterState, engine: SearchEngine, now: Instant) -> bool {
    !state
        .cooldowns
        .get(&engine)
        .is_some_and(|cooldown_end| now < *cooldown_end)
}

/// Pick the next engine to try for `choice` given the current router state
/// and the persisted pacing timestamps.
///
/// Pure state-machine helper (no I/O): with [`EngineChoice::Auto`] returns the
/// first engine in [`SearchEngine::PRIORITY`] order that is neither cooling
/// down nor over budget **and** whose persisted minimum interval has elapsed;
/// with [`EngineChoice::Pin`] returns the pinned engine unconditionally (the
/// caller is responsible for enforcing cooldown and pacing).
fn next_engine(
    state: &RouterState,
    pacing: &PacingStore,
    choice: &EngineChoice,
) -> Result<SearchEngine, SearchEngineError> {
    match choice {
        EngineChoice::Pin(engine) => Ok(*engine),
        EngineChoice::Auto => {
            let now = Instant::now();
            let now_ms = unix_now_ms();
            for engine in SearchEngine::PRIORITY {
                if engine_ready(state, engine, now)
                    && pacing_ready(
                        pacing.last_call_ms(engine),
                        min_interval(engine),
                        now_ms,
                    )
                {
                    return Ok(engine);
                }
            }
            Err(SearchEngineError::Unavailable {
                engine: SearchEngine::PRIORITY[0],
                detail: "all engines cooling down or over budget".to_string(),
            })
        }
    }
}

/// Pick the next engine for auto mode and **reserve** it in one atomic step.
///
/// The pick and the pre-dispatch pacing stamp (record) happen under a single
/// lock acquisition of `state` then `pacing` — the same lock order used
/// everywhere in this module — so two concurrent picks can never select the
/// same engine before either has stamped it: the second pick sees the first's
/// reservation and moves on. No await happens inside the lock scope, so the
/// future stays `Send` for the orchestrator's JoinSet::spawn.
fn pick_and_reserve(
    state: &Mutex<RouterState>,
    pacing: &Mutex<PacingStore>,
) -> Result<SearchEngine, SearchEngineError> {
    let state = state.lock().unwrap_or_else(|e| e.into_inner());
    let mut pacing = pacing.lock().unwrap_or_else(|e| e.into_inner());
    let engine = next_engine(&state, &pacing, &EngineChoice::Auto)?;
    pacing.record(engine, unix_now_ms());
    Ok(engine)
}

/// Wait (bounded by `deadline`) for any engine to become eligible in auto
/// mode.
///
/// Every pass re-checks eligibility; the first eligible engine is returned —
/// and reserved atomically with the pick (see [`pick_and_reserve`]), so a
/// racing sibling task's next poll sees the reservation. Since pacing state
/// only changes via this process's own dispatches, the loop is **event-driven**:
/// it awaits [`Notify::notified`] so a concurrent dispatch that records pacing
/// (or a cooldown) wakes it up immediately instead of busy-polling. The wait is
/// still bounded by the earliest eligible engine (capped at `deadline`) via a
/// timeout on the notify, so a wakeup that never comes still sleeps the full
/// remaining interval. Once `deadline` passes, `unavailable` (the original "all
/// engines busy" error) is returned so the per-query orchestrator timeout is
/// still respected.
async fn wait_for_available_engine(
    state: &Mutex<RouterState>,
    pacing: &Mutex<PacingStore>,
    notify: &Notify,
    deadline: Instant,
    unavailable: SearchEngineError,
) -> Result<SearchEngine, SearchEngineError> {
    loop {
        // Every pass atomically picks **and reserves** the first eligible
        // engine (pick + stamp under one lock scope). When two tasks race,
        // the loser's next poll sees the winner's reservation and picks a
        // different engine — or keeps waiting.
        match pick_and_reserve(state, pacing) {
            Ok(engine) => return Ok(engine),
            Err(_) => {
                // Milliseconds until the earliest engine becomes eligible
                // under the in-memory cooldowns and persisted pacing rules.
                let remaining = {
                    let state_guard = state.lock().unwrap_or_else(|e| e.into_inner());
                    let pacing_guard = pacing.lock().unwrap_or_else(|e| e.into_inner());
                    earliest_available_remaining_ms(
                        &state_guard,
                        &pacing_guard,
                        Instant::now(),
                        unix_now_ms(),
                    )
                };
                let now = Instant::now();
                if now >= deadline {
                    return Err(unavailable);
                }
                // Event-driven wait: wake early if a concurrent dispatch
                // records pacing/cooldown (notify_waiters), otherwise sleep
                // the full remaining time (capped at the deadline). No busy
                // polling — the notify fires exactly when pacing changes.
                let step = match remaining {
                    Some(ms) => Duration::from_millis(ms),
                    None => WAIT_POLL,
                };
                let wait = step.min(deadline.saturating_duration_since(now));
                let _ = tokio::time::timeout(wait, notify.notified()).await;
            }
        }
    }
}

/// Unix timestamp in milliseconds (0 if the system clock is before the
/// epoch, which cannot meaningfully pace anything).
fn unix_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Remaining milliseconds until `engine`'s minimum interval has elapsed since
/// its last recorded call, or `None` when it may be dispatched right away.
///
/// Pure decision helper shared by auto mode (skip) and pin mode (wait):
/// `last_call_ms` is the persisted last-call timestamp, `min_interval_ms` the
/// engine's minimum interval, `now_ms` the current unix millis.
fn pacing_remaining_ms(
    last_call_ms: Option<u64>,
    min_interval_ms: u64,
    now_ms: u64,
) -> Option<u64> {
    let last_call = last_call_ms?;
    let elapsed = now_ms.saturating_sub(last_call);
    if elapsed >= min_interval_ms {
        None
    } else {
        Some(min_interval_ms - elapsed)
    }
}

/// Whether `engine` may be dispatched under the persisted pacing rules.
fn pacing_ready(last_call_ms: Option<u64>, min_interval: Duration, now_ms: u64) -> bool {
    pacing_remaining_ms(last_call_ms, min_interval.as_millis() as u64, now_ms).is_none()
}

/// Milliseconds until the earliest engine becomes eligible under the
/// in-memory cooldowns and persisted pacing rules — the min over engines of
/// (remaining minimum interval, persisted cooldown expiry, in-memory cooldown
/// expiry), each offset by already-elapsed time. `None` when every engine is
/// already eligible.
///
/// Pure decision helper for the auto-mode wait loop: a `Some(0)` can never
/// occur (a ready engine contributes `None`, which wins as the minimum).
fn earliest_available_remaining_ms(
    state: &RouterState,
    pacing: &PacingStore,
    now: Instant,
    now_ms: u64,
) -> Option<u64> {
    let mut earliest: Option<u64> = None;
    for engine in SearchEngine::PRIORITY {
        let interval_remaining = pacing_remaining_ms(
            pacing.last_call_ms(engine),
            min_interval(engine).as_millis() as u64,
            now_ms,
        );
        // A cooldown that has already expired does not block the engine.
        let persisted_cooldown = pacing
            .cooldown_until_ms(engine)
            .map(|until| until.saturating_sub(now_ms))
            .filter(|remaining| *remaining > 0);
        let mem_cooldown = state
            .cooldowns
            .get(&engine)
            .map(|end| end.saturating_duration_since(now).as_millis() as u64)
            .filter(|remaining| *remaining > 0);
        let cooldown_remaining = match (persisted_cooldown, mem_cooldown) {
            (None, None) => None,
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (Some(a), Some(b)) => Some(a.max(b)),
        };
        let remaining = match (interval_remaining, cooldown_remaining) {
            (None, None) => continue, // eligible right now
            (Some(ms), None) => ms,
            (None, Some(ms)) => ms,
            (Some(interval_ms), Some(cooldown_ms)) => interval_ms.max(cooldown_ms),
        };
        earliest = Some(earliest.map_or(remaining, |e| e.min(remaining)));
    }
    earliest
}

/// Record the outcome of a dispatched query in `state`.
///
/// Pure state-machine helper (no I/O): rate-limits put the engine into a
/// 5-minute cooldown, captchas into a 30-minute cooldown. Every other outcome
/// (success or non-block error) leaves the in-memory state untouched — the
/// query budget is stamped into the [`PacingStore`] by the caller instead.
/// Returns the cooldown the caller must persist — `(engine, until-ms)` — for
/// rate-limit/captcha outcomes so the block survives across CLI invocations;
/// `None` for every other outcome.
fn record_outcome(
    state: &mut RouterState,
    engine: SearchEngine,
    result: &Result<Vec<EngineSearchResult>, SearchEngineError>,
) -> Option<(SearchEngine, u64)> {
    let now = Instant::now();
    let now_ms = unix_now_ms();
    match result {
        Err(SearchEngineError::RateLimited { .. }) => {
            state.cooldowns.insert(engine, now + RATE_LIMITED_COOLDOWN);
            Some((engine, now_ms + RATE_LIMITED_COOLDOWN.as_millis() as u64))
        }
        Err(SearchEngineError::Captcha { .. }) => {
            state.cooldowns.insert(engine, now + CAPTCHA_COOLDOWN);
            Some((engine, now_ms + CAPTCHA_COOLDOWN.as_millis() as u64))
        }
        Ok(_) | Err(_) => None,
    }
}

/// Rebuild the in-memory cooldown map from persisted cooldown state.
///
/// Only entries whose until-ms is still in the future survive; expired
/// entries are dropped. (The store itself keeps expired entries until the
/// next write — harmless, since they are never consulted again.)
fn seed_cooldowns(
    pacing: &PacingStore,
    now: Instant,
    now_ms: u64,
) -> HashMap<SearchEngine, Instant> {
    let mut cooldowns = HashMap::new();
    for engine in SearchEngine::PRIORITY {
        let Some(until_ms) = pacing.cooldown_until_ms(engine) else {
            continue;
        };
        if until_ms > now_ms {
            let remaining = Duration::from_millis(until_ms - now_ms);
            cooldowns.insert(engine, now + remaining);
        }
    }
    cooldowns
}

/// Auto-mode fallback accumulator (pure; unit-tested).
///
/// As the fallback loop walks [`SearchEngine::PRIORITY`], this tracks engine
/// failures and legitimate empty result sets. An engine answering `Ok` with
/// an empty vector is a *healthy* answer ("zero results for this query on
/// this engine") and must not mask the remaining engines — the loop keeps
/// going. Only when every engine has been tried without a single failure
/// does the query genuinely have zero results.
#[derive(Debug, Default)]
struct FallbackAccum {
    /// Errors from failed engines; non-empty ⇒ `AllEnginesFailed` verdict.
    errors: Vec<SearchEngineError>,
    /// Whether any engine returned a legitimate empty result set.
    saw_empty_ok: bool,
}

impl FallbackAccum {
    /// Record one engine outcome: `Some(results)` serves non-empty results
    /// immediately; `None` keeps the fallback loop going (empty `Ok` or
    /// `Err`).
    fn record(
        &mut self,
        outcome: Result<Vec<EngineSearchResult>, SearchEngineError>,
    ) -> Option<Vec<EngineSearchResult>> {
        match outcome {
            Ok(results) if results.is_empty() => {
                self.saw_empty_ok = true;
                None
            }
            Ok(results) => Some(results),
            Err(e) => {
                self.errors.push(e);
                None
            }
        }
    }

    /// Final verdict once every available engine has been tried: no engine
    /// failed but none produced results ⇒ `Ok(vec![])`; any failure ⇒
    /// [`SearchEngineError::AllEnginesFailed`].
    fn exhausted(self) -> Result<Vec<EngineSearchResult>, SearchEngineError> {
        if !self.errors.is_empty() {
            Err(SearchEngineError::AllEnginesFailed(self.errors))
        } else {
            Ok(Vec::new())
        }
    }
}

/// The query actually sent to `engine`'s backend: the raw query rewritten
/// per-engine by [`technique::rewrite`], stripping operators the engine does
/// not support. No-operator queries come back byte-identical.
fn effective_query(engine: SearchEngine, query: &str) -> String {
    technique::rewrite(query, engine).into_owned()
}

/// Multi-engine search router with fallback, cooldowns, and query budgets.
pub struct SearchRouter {
    /// Plain-HTTP Brave backend (always available).
    brave: BraveBackend,
    /// Plain-HTTP Bing backend (RSS endpoint; always available).
    bing: BingBackend,
    /// CDP Google backend; only present when a browser session was supplied.
    google: Option<GoogleBackend>,
    /// Shared cooldown/budget state, guarded for concurrent queries.
    state: Mutex<RouterState>,
    /// Persistent per-engine last-call timestamps, guarded for concurrent
    /// queries; enforces minimum intervals across CLI invocations.
    pacing: Arc<Mutex<PacingStore>>,
    /// Wakes the auto-mode wait loop when pacing/cooldown state changes, so it
    /// reacts immediately instead of busy-polling.
    notify: Notify,
}

impl SearchRouter {
    /// Create a router with all four backends.
    ///
    /// Bing speaks plain HTTP (RSS endpoint) and is always constructed; the
    /// CDP-backed Google backend is constructed only when `session` is
    /// `Some`. Without a browser session Google fails with
    /// [`SearchEngineError::Unavailable`] and is skipped in Auto mode.
    ///
    /// Pacing is fully in-memory and **process-wide**: this router shares the
    /// single [`global_pacing`] store with every other router built in this
    /// process, so minimum-interval pacing and cooldowns persist across router
    /// instances (e.g. when the orchestrator rebuilds a router per harvest
    /// call). Cooldowns still in the future are seeded into the in-memory map;
    /// expired ones are dropped.
    pub fn new(session: Option<Arc<gthings_cdp::Session>>) -> Self {
        Self::with_pacing(session, global_pacing().clone())
    }

    /// Create a router sharing the given `pacing` store.
    ///
    /// Test-friendly constructor: lets tests inject an isolated
    /// [`PacingStore`] instead of the process-wide global, avoiding
    /// cross-test state pollution. Production code should use [`Self::new`],
    /// which shares the global store.
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
            brave: BraveBackend,
            bing: BingBackend::new(),
            google: session.map(GoogleBackend::new),
            state: Mutex::new(state),
            // In-memory last-call timestamps enforce minimum intervals for
            // the lifetime of this process.
            pacing,
            notify: Notify::new(),
        }
    }

    /// Dispatch `query` to the concrete backend for `engine`.
    ///
    /// The raw query is first rewritten per-engine (see [`technique`]) so
    /// unsupported operators (`AROUND`, parens on Bing, ...) never reach the
    /// backend; no-operator queries pass through byte-for-byte. Browser
    /// backends manage their own background tabs internally (the session is
    /// held inside the backend); this only awaits their `search`.
    async fn dispatch(
        &self,
        engine: SearchEngine,
        query: &str,
        count: usize,
    ) -> Result<Vec<EngineSearchResult>, SearchEngineError> {
        let effective_query = effective_query(engine, query);
        match engine {
            SearchEngine::DuckDuckGo => Err(SearchEngineError::Unavailable {
                engine,
                detail: "DDG removed: bot detection / TLS fingerprinting blocks this IP"
                    .to_string(),
            }),
            SearchEngine::Brave => self.brave.search(&effective_query, count).await,
            SearchEngine::Bing => self.bing.search(&effective_query, count).await,
            SearchEngine::Google => match &self.google {
                Some(google) => google.search(&effective_query, count).await,
                None => Err(SearchEngineError::Unavailable {
                    engine,
                    detail: "no browser session; Google backend not constructed".to_string(),
                }),
            },
        }
    }

    /// Run `query`, honoring `choice`.
    ///
    /// [`EngineChoice::Auto`]: try engines in priority order, skipping any
    /// engine that is cooling down or over budget; fall back on failure — or
    /// on a legitimate empty result set — until an engine returns results or
    /// all are exhausted.
    ///
    /// [`EngineChoice::Pin`]: respect the pinned engine's cooldown (hard
    /// error) and minimum interval (wait out the remainder — polite
    /// throttling), dispatch it, record the outcome, and return its result
    /// directly — no fallback, no aggregation.
    pub async fn search_with_fallback(
        &self,
        query: &str,
        count: usize,
        choice: EngineChoice,
    ) -> Result<Vec<EngineSearchResult>, SearchEngineError> {
        match choice {
            EngineChoice::Pin(engine) => self.search_pinned(query, count, engine).await,
            EngineChoice::Auto => self.search_auto(query, count).await,
        }
    }

    /// Auto mode: priority-order fallback across all available engines.
    ///
    /// An engine answering `Ok` with an empty vector is a healthy zero-result
    /// answer; it does not end the search (it could mask a still-healthy
    /// engine), so the loop keeps going. If every engine answers empty and
    /// none failed, the query genuinely has zero results.
    async fn search_auto(
        &self,
        query: &str,
        count: usize,
    ) -> Result<Vec<EngineSearchResult>, SearchEngineError> {
        let mut accum = FallbackAccum::default();
        loop {
            let engine = {
                // Pick **and reserve** the engine atomically: the pick and the
                // in-memory last-call stamp all happen under one lock scope
                // inside pick_and_reserve, so two concurrent searches can
                // never both pick the same engine before either has stamped
                // it. No await inside the scope, so the future stays `Send`
                // for the orchestrator's JoinSet::spawn.
                match pick_and_reserve(&self.state, &self.pacing) {
                    Ok(engine) => engine,
                    Err(e) if accum.errors.is_empty() && !accum.saw_empty_ok => {
                        // All engines busy: don't hard-fail — wait (bounded by
                        // MAX_AUTO_WAIT) for one to become eligible.
                        match wait_for_available_engine(
                            &self.state,
                            &self.pacing,
                            &self.notify,
                            Instant::now() + MAX_AUTO_WAIT,
                            e,
                        )
                        .await
                        {
                            Ok(engine) => engine,
                            Err(e) => return Err(e),
                        }
                    }
                    Err(_) => return accum.exhausted(),
                }
            };
            let result = self.dispatch(engine, query, count).await;
            self.record_dispatch_outcome(engine, &result);

            if let Some(results) = accum.record(result) {
                return Ok(results);
            }
        }
    }

    /// Pin mode: single dispatch on the pinned engine, cooldown- and
    /// pacing-aware.
    ///
    /// Rate-limit/captcha cooldowns remain a hard error (unchanged). The
    /// engine's *minimum interval* is enforced as polite throttling: when the
    /// in-memory last-call timestamp shows the interval has not yet elapsed,
    /// this waits out the remainder with `tokio::time::sleep` before
    /// dispatching instead of erroring.
    async fn search_pinned(
        &self,
        query: &str,
        count: usize,
        engine: SearchEngine,
    ) -> Result<Vec<EngineSearchResult>, SearchEngineError> {
        // Hard cooldowns (recent rate-limit/captcha block) still refuse
        // immediately — sleeping through a 5- or 30-minute block would be
        // worse than erroring.
        {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let now = Instant::now();
            if state
                .cooldowns
                .get(&engine)
                .is_some_and(|cooldown_end| now < *cooldown_end)
            {
                return Err(SearchEngineError::Unavailable {
                    engine,
                    detail: "engine cooling down (recent rate-limit or captcha)".to_string(),
                });
            }
        }

        // Pacing: wait out the in-memory minimum interval before dispatching.
        let remaining = {
            let pacing = self.pacing.lock().unwrap_or_else(|e| e.into_inner());
            pacing_remaining_ms(
                pacing.last_call_ms(engine),
                min_interval(engine).as_millis() as u64,
                unix_now_ms(),
            )
        };
        if let Some(remaining) = remaining {
            tracing::info!(
                "pacing: waiting {remaining}ms before pinned {} query (minimum interval not yet elapsed)",
                engine.as_str()
            );
            tokio::time::sleep(Duration::from_millis(remaining)).await;
        }

        // Stamp the dispatch *start* before the backend call (same rationale
        // as in search_auto: a timeout during dispatch must still leave a
        // timestamp so the next invocation is paced).
        self.record_pacing(engine);
        let result = self.dispatch(engine, query, count).await;
        self.record_dispatch_outcome(engine, &result);
        result
    }

    /// Record the outcome of a dispatched query: emit tracing, update the
    /// in-memory cooldown state, and stamp the persisted pacing store.
    ///
    /// Shared by auto and pin modes so the post-dispatch bookkeeping stays in
    /// one place. When the outcome carries a cooldown (rate-limit/captcha),
    /// the cooldown and the last-call stamp are written under a single
    /// [`PacingStore`] lock acquisition.
    fn record_dispatch_outcome(
        &self,
        engine: SearchEngine,
        result: &Result<Vec<EngineSearchResult>, SearchEngineError>,
    ) {
        match result {
            Ok(_) => tracing::debug!("served query via {}", engine.as_str()),
            Err(e) => tracing::warn!("engine {} failed: {e}", engine.as_str()),
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let cooldown = record_outcome(&mut state, engine, result);
        drop(state);
        match cooldown {
            Some((engine, until_ms)) => self.record_cooldown_and_pacing(engine, until_ms),
            None => self.record_pacing(engine),
        }
    }

    /// Record a cooldown and the last-call timestamp for `engine` under a
    /// single [`PacingStore`] lock acquisition (they are always written
    /// back-to-back after a dispatch).
    fn record_cooldown_and_pacing(&self, engine: SearchEngine, until_ms: u64) {
        let now_ms = unix_now_ms();
        let mut pacing = self.pacing.lock().unwrap_or_else(|e| e.into_inner());
        pacing.record_cooldown(engine, until_ms);
        pacing.record(engine, now_ms);
        drop(pacing);
        // Wake any auto-mode wait loop: pacing/cooldown state just changed.
        self.notify.notify_waiters();
    }

    /// Record the last-call timestamp for `engine` after a dispatch.
    fn record_pacing(&self, engine: SearchEngine) {
        let now_ms = unix_now_ms();
        let mut pacing = self.pacing.lock().unwrap_or_else(|e| e.into_inner());
        pacing.record(engine, now_ms);
        drop(pacing);
        // Wake any auto-mode wait loop: pacing state just changed.
        self.notify.notify_waiters();
    }
}

/// Whether `url` is a Google translate/redirect wrapper that must never be
/// surfaced as an organic result: the `translate.google.com/translate`
/// proxy, its `*.translate.goog` host, or Google's `/url?q=` redirect
/// wrapper.
pub(crate) fn is_translate_wrapper_url(url: &str) -> bool {
    let host = gthings_common::extract_host(url)
        .unwrap_or_default()
        .to_lowercase();
    // The translate.google.com proxy and any *.translate.goog host.
    if host == "translate.google.com" || host.ends_with(".translate.goog") {
        return true;
    }
    // Google's /url?q= redirect wrapper: host is google.com and the path is
    // exactly "/url" with a `q` query parameter.
    if host == "google.com" || host == "www.google.com" {
        let path = url.split('?').next().unwrap_or(url);
        if path.ends_with("/url") && url.contains("?q=") {
            return true;
        }
    }
    false
}

/// Whether `text` (a title or snippet) contains any character in a non-Latin
/// script that surfaces as junk for English queries: CJK Unified Ideographs
/// (U+4E00–U+9FFF), Hiragana (U+3040–U+309F), Katakana (U+30A0–U+30FF), Hangul
/// (U+AC00–U+D7AF), plus Latin Extended Additional (U+1E00–U+1EFF), Latin-1
/// diacritics (U+00C0–U+00FF), Cyrillic (U+0400–U+04FF), Greek (U+0370–U+03FF),
/// Arabic (U+0600–U+06FF), Thai (U+0E00–U+0E7F), and Devanagari (U+0900–U+097F).
/// Localized (non-English) results for English queries surface as junk; their
/// titles and snippets carry these scripts. Applied to both the title and the
/// snippet so a non-English snippet alone (e.g. a Chinese description under an
/// English title) is still rejected.
pub(crate) fn has_non_latin_script(text: &str) -> bool {
    text.chars().any(|c| {
        matches!(c as u32,
            0x3040..=0x309F   // Hiragana
            | 0x30A0..=0x30FF // Katakana
            | 0x4E00..=0x9FFF // CJK Unified Ideographs
            | 0xAC00..=0xD7AF // Hangul
            | 0x1E00..=0x1EFF // Latin Extended Additional (Vietnamese etc.)
            | 0x00C0..=0x00FF // Latin-1 diacritics
            | 0x0400..=0x04FF // Cyrillic
            | 0x0370..=0x03FF // Greek
            | 0x0600..=0x06FF // Arabic
            | 0x0E00..=0x0E7F // Thai
            | 0x0900..=0x097F // Devanagari
        )
    })
}

/// Domains that are dictionary/definition sites whose results are junk for
/// general English queries: they answer "what does X mean" rather than
/// substantive content. Matched on the exact host or any subdomain.
const DICTIONARY_DOMAINS: [&str; 8] = [
    "cambridge.org",
    "merriam-webster.com",
    "dictionary.com",
    "scribbr.com",
    "thefreedictionary.com",
    "vocabulary.com",
    "collinsdictionary.com",
    "oxfordlearnersdictionaries.com",
];

/// Whether `url`/`title`/`snippet` indicate a dictionary-definition page that
/// should be filtered as junk for general English queries.
///
/// Rejects results from known dictionary/definition domains, plus results whose
/// title is a single word followed by "definition" (e.g. "Rust definition") or
/// whose snippet contains "definition of". Deliberately narrow so legitimate
/// content that merely mentions a definition is not over-filtered.
pub(crate) fn is_dictionary_junk(url: &str, title: &str, snippet: &str) -> bool {
    let host = gthings_common::extract_host(url)
        .unwrap_or_default()
        .to_lowercase();
    if DICTIONARY_DOMAINS
        .iter()
        .any(|d| host == *d || host.ends_with(&format!(".{d}")))
    {
        return true;
    }
    let title_lower = title.to_lowercase();
    let snippet_lower = snippet.to_lowercase();
    // Title is a single word followed by "definition" (e.g. "Rust definition").
    if let Some(word) = title_lower.strip_suffix(" definition") {
        let word = word.trim();
        if !word.is_empty() && !word.contains(char::is_whitespace) {
            return true;
        }
    }
    // Snippet explicitly defines a term.
    if snippet_lower.contains("definition of") {
        return true;
    }
    false
}

/// Convert normalized engine results into crate-level [`SearchResult`]s.
///
/// Filters junk URLs, translate/redirect wrappers, non-Latin (localized)
/// titles, `#:~:text=` fragments, and empty snippets; dedups by base URL
/// (before the first `#`); re-numbers positions 1-based; trims titles;
/// attaches per-result provenance (`source_url` = the result URL, `method` =
/// [`ExtractionMethod::Search`]); and rounds domain authority to two
/// decimals. `source_url` is the query's originating context used for
/// tracing only — provenance carries the result URL, matching
/// `crate::search` semantics.
// Consumed by the search facade and harvest phase_search, which are wired to
// this crate-internal helper separately.
pub(crate) fn map_engine_results(
    results: Vec<EngineSearchResult>,
    source_url: &str,
    duration_ms: u64,
) -> Vec<SearchResult> {
    // Phase 1: filter out junk / wrapper / non-Latin / empty-title /
    // empty-snippet results.
    let mut survivors: Vec<EngineSearchResult> = results
        .into_iter()
        .filter(|r| {
            !r.url.contains("#:~:text=")
                && !is_translate_wrapper_url(&r.url)
                && !has_non_latin_script(&r.title)
                && !has_non_latin_script(&r.snippet)
                && !is_dictionary_junk(&r.url, &r.title, &r.snippet)
                && !crate::harvest::is_junk_url(&r.url)
                && !r.title.trim().is_empty()
                && !r.snippet.trim().is_empty()
        })
        .collect();

    // Phase 2: dedup by normalized base URL and by normalized title. The
    // normalized keys are owned Strings, so they outlive the survivors.
    let mut seen_bases: HashSet<String> = HashSet::new();
    let mut seen_titles: HashSet<String> = HashSet::new();
    let mut keep: Vec<usize> = Vec::with_capacity(survivors.len());
    for (idx, r) in survivors.iter().enumerate() {
        let base = normalize_base_url(&r.url);
        let title = normalize_title(&r.title);
        if seen_bases.insert(base) && seen_titles.insert(title) {
            keep.push(idx);
        }
    }

    // Phase 3: move the kept survivors into results (removing in reverse so
    // earlier indices stay valid), then restore original order and renumber.
    let mut mapped: Vec<SearchResult> = Vec::with_capacity(keep.len());
    let agent = GTHINGS_AGENT.to_string();
    for idx in keep.into_iter().rev() {
        let r = survivors.remove(idx);
        let host = gthings_common::extract_host(&r.url).unwrap_or_default();
        let authority =
            (gthings_extraction::domain_authority(&host) as f64 * 100.0).round() / 100.0;
        let source_type = classify_source_type(&r.url);
        mapped.push(SearchResult {
            title: collapse_whitespace(&r.title),
            url: r.url.clone(),
            snippet: collapse_whitespace(&r.snippet),
            position: 0,
            provenance: Provenance {
                source_url: r.url,
                method: ExtractionMethod::Search,
                agent: agent.clone(),
                accessed_at: chrono::Utc::now(),
                duration_ms,
                derived_from: None,
            },
            domain_authority: authority,
            source_type,
        });
    }
    mapped.reverse();
    for (i, r) in mapped.iter_mut().enumerate() {
        r.position = i + 1;
    }

    tracing::debug!(
        "mapped {} engine results (context {source_url:?}, {duration_ms}ms)",
        mapped.len()
    );
    mapped
}

/// Normalize a title for dedup and uniform cleaning: trim and collapse runs of
/// whitespace into single spaces, then lowercase. Applied consistently to
/// titles from every engine so Google's own heuristic cannot diverge.
fn normalize_title(title: &str) -> String {
    collapse_whitespace(title).to_lowercase()
}

/// Normalize a result URL into a canonical base key for dedup. Delegates to
/// the shared [`gthings_common::dedup_key`] so there is a single source of
/// truth for URL base normalization across the engine layer.
fn normalize_base_url(url: &str) -> String {
    gthings_common::dedup_key(url)
}

/// Classify a result URL into a coarse `source_type` for citation metadata:
/// `github` for GitHub, `paper` for arXiv, `pdf` for direct PDF links, and
/// `web` for everything else.
fn classify_source_type(url: &str) -> String {
    let host = gthings_common::extract_host(url).unwrap_or_default().to_lowercase();
    if host == "github.com" || host.ends_with(".github.com") {
        "github".to_string()
    } else if host == "arxiv.org" || host.ends_with(".arxiv.org") {
        "paper".to_string()
    } else if url.to_lowercase().ends_with(".pdf") {
        "pdf".to_string()
    } else {
        "web".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

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
        }
    }

    #[test]
    fn auto_picks_priority_first_when_cold() {
        let state = cold_state();
        assert_eq!(
            next_engine(&state, &cold_pacing(), &EngineChoice::Auto).unwrap(),
            SearchEngine::Brave
        );
    }

    #[test]
    fn falls_back_after_rate_limited() {
        let mut state = cold_state();
        let error = Err(SearchEngineError::RateLimited {
            engine: SearchEngine::Brave,
            detail: "HTTP 429".to_string(),
        });
        record_outcome(&mut state, SearchEngine::Brave, &error);
        assert_eq!(
            next_engine(&state, &cold_pacing(), &EngineChoice::Auto).unwrap(),
            SearchEngine::Bing
        );
    }

    #[test]
    fn skips_engine_in_captcha_cooldown() {
        let mut state = cold_state();
        state
            .cooldowns
            .insert(SearchEngine::Brave, Instant::now() + Duration::from_secs(30 * 60));
        assert_eq!(
            next_engine(&state, &cold_pacing(), &EngineChoice::Auto).unwrap(),
            SearchEngine::Bing
        );
    }

    #[test]
    fn skips_engine_over_budget() {
        // Over-budget is enforced via the persisted pacing store (the single
        // source of truth for query budgets): a just-recorded Brave call makes
        // auto mode skip it and fall back to Bing.
        let state = cold_state();
        let now_ms = unix_now_ms();
        let mut pacing = cold_pacing();
        pacing.record(SearchEngine::Brave, now_ms);
        assert_eq!(
            next_engine(&state, &pacing, &EngineChoice::Auto).unwrap(),
            SearchEngine::Bing
        );
    }

    #[test]
    fn all_engines_cooled_yields_unavailable() {
        let mut state = cold_state();
        let future = Instant::now() + Duration::from_secs(3600);
        for engine in SearchEngine::PRIORITY {
            state.cooldowns.insert(engine, future);
        }
        let err = next_engine(&state, &cold_pacing(), &EngineChoice::Auto).unwrap_err();
        match err {
            SearchEngineError::Unavailable { engine, detail } => {
                assert_eq!(engine, SearchEngine::Brave);
                assert!(detail.contains("all engines cooling down or over budget"));
            }
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    #[test]
    fn pin_forces_pinned_engine_even_when_not_first() {
        let state = cold_state();
        assert_eq!(
            next_engine(&state, &cold_pacing(), &EngineChoice::Pin(SearchEngine::Google)).unwrap(),
            SearchEngine::Google
        );
        assert_eq!(
            next_engine(&state, &cold_pacing(), &EngineChoice::Pin(SearchEngine::Bing)).unwrap(),
            SearchEngine::Bing
        );
    }

    #[test]
    fn pacing_remaining_none_without_recorded_call() {
        assert_eq!(pacing_remaining_ms(None, 4000, unix_now_ms()), None);
    }

    #[test]
    fn pacing_remaining_none_once_interval_elapsed() {
        // 5s elapsed, Brave's 30s interval has not passed, but 5s > 4s so test with 4s interval.
        assert_eq!(pacing_remaining_ms(Some(1_000_000), 4000, 1_005_000), None);
        // Exactly at the boundary → no wait.
        assert_eq!(pacing_remaining_ms(Some(1_000_000), 4000, 1_004_000), None);
    }

    #[test]
    fn pacing_remaining_waits_until_interval_elapsed() {
        // 3s elapsed, 4s interval → wait 1s.
        assert_eq!(pacing_remaining_ms(Some(1_000_000), 4000, 1_003_000), Some(1000));
        // Zero elapsed → wait the full interval.
        assert_eq!(pacing_remaining_ms(Some(1_000_000), 4000, 1_000_000), Some(4000));
        // A timestamp in the future (clock skew) → wait the full interval.
        assert_eq!(pacing_remaining_ms(Some(2_000_000), 4000, 1_000_000), Some(4000));
    }

    #[test]
    fn auto_skips_engine_with_recent_persisted_call() {
        // Brave was called 1s ago (60s interval not elapsed) — the persisted
        // pacing state must make auto mode skip it just like an over-budget
        // engine, falling back to Bing.
        let state = cold_state();
        let now_ms = unix_now_ms();
        let mut pacing = cold_pacing();
        pacing.record(SearchEngine::Brave, now_ms - 1000);
        assert_eq!(
            next_engine(&state, &pacing, &EngineChoice::Auto).unwrap(),
            SearchEngine::Bing
        );
    }

    #[test]
    fn auto_skips_engine_until_persisted_interval_elapses() {
        let state = cold_state();
        let now_ms = unix_now_ms();
        let mut pacing = cold_pacing();
        pacing.record(SearchEngine::Brave, now_ms - 1000);
        // Still within the 60s Brave interval: skipped.
        assert_ne!(
            next_engine(&state, &pacing, &EngineChoice::Auto).unwrap(),
            SearchEngine::Brave
        );
        // Once 60s have elapsed the engine is eligible again.
        pacing.record(SearchEngine::Brave, now_ms - 61_000);
        assert_eq!(
            next_engine(&state, &pacing, &EngineChoice::Auto).unwrap(),
            SearchEngine::Brave
        );
    }

    #[test]
    fn earliest_remaining_takes_min_across_engines() {
        // Synthetic timeline: Bing 0s into its 1s interval, Brave 30s into
        // its 60s interval, Google fully elapsed (eligible).
        let now_ms = 1_000_000;
        let mut pacing = cold_pacing();
        pacing.record(SearchEngine::Bing, now_ms);
        pacing.record(SearchEngine::Brave, now_ms - 30_000);
        pacing.record(SearchEngine::Google, now_ms - 6_000);
        assert_eq!(
            earliest_available_remaining_ms(&cold_state(), &pacing, Instant::now(), now_ms),
            Some(1000),
            "min remaining across engines wins (Bing's 1s interval)"
        );
    }

    #[test]
    fn earliest_remaining_none_when_all_engines_eligible() {
        let now_ms = 1_000_000;
        assert_eq!(
            earliest_available_remaining_ms(&cold_state(), &cold_pacing(), Instant::now(), now_ms),
            None,
            "cold pacing: every engine may be dispatched right away"
        );
        // Interval fully elapsed on every engine ⇒ still None.
        let mut pacing = cold_pacing();
        pacing.record(SearchEngine::Brave, now_ms - 60_000);
        pacing.record(SearchEngine::Bing, now_ms - 1_000);
        pacing.record(SearchEngine::Google, now_ms - 6_000);
        assert_eq!(
            earliest_available_remaining_ms(&cold_state(), &pacing, Instant::now(), now_ms),
            None
        );
    }

    #[test]
    fn earliest_remaining_includes_persisted_cooldowns() {
        let now_ms = 1_000_000;
        let mut pacing = cold_pacing();
        // Bing: 1s interval elapsed → eligible; Google: 10s of persisted
        // cooldown left (and 5s of interval left — the cooldown dominates).
        pacing.record(SearchEngine::Bing, now_ms - 1_000);
        pacing.record(SearchEngine::Google, now_ms - 1_000);
        pacing.record_cooldown(SearchEngine::Google, now_ms + 10_000);
        assert_eq!(
            earliest_available_remaining_ms(&cold_state(), &pacing, Instant::now(), now_ms),
            Some(10_000)
        );
        // An expired cooldown no longer blocks.
        pacing.record_cooldown(SearchEngine::Google, now_ms - 1_000);
        assert_eq!(
            earliest_available_remaining_ms(&cold_state(), &pacing, Instant::now(), now_ms),
            Some(5_000),
            "Google still within its 6s interval: 5s left"
        );
    }

    #[test]
    fn earliest_remaining_includes_in_memory_cooldowns() {
        // An in-memory cooldown (rate-limit/captcha block) must be folded into
        // the earliest-remaining computation so the wait loop sleeps
        // accurately instead of only consulting persisted pacing.
        let now = Instant::now();
        let now_ms = 1_000_000;
        let mut state = cold_state();
        state
            .cooldowns
            .insert(SearchEngine::Brave, now + Duration::from_secs(30));
        assert_eq!(
            earliest_available_remaining_ms(&state, &cold_pacing(), now, now_ms),
            Some(30_000),
            "in-memory cooldown dominates the earliest remaining"
        );
        // An expired in-memory cooldown no longer blocks.
        state
            .cooldowns
            .insert(SearchEngine::Brave, now - Duration::from_secs(1));
        assert_eq!(
            earliest_available_remaining_ms(&state, &cold_pacing(), now, now_ms),
            None
        );
    }

    #[tokio::test]
    async fn all_blocked_waits_bounded_then_returns_unavailable() {
        // Every engine cooled down for an hour: the wait loop must spin for
        // the (tiny, injected) deadline and then surface the original
        // Unavailable error — no hard-fail before the deadline.
        let state = Mutex::new({
            let mut state = cold_state();
            let future = Instant::now() + Duration::from_secs(3600);
            for engine in SearchEngine::PRIORITY {
                state.cooldowns.insert(engine, future);
            }
            state
        });
        let pacing = Mutex::new(cold_pacing());
        let notify = Notify::new();
        let deadline = Instant::now() + Duration::from_millis(30);
        let err = wait_for_available_engine(
            &state,
            &pacing,
            &notify,
            deadline,
            SearchEngineError::Unavailable {
                engine: SearchEngine::Brave,
                detail: "all engines cooling down or over budget".to_string(),
            },
        )
        .await
        .unwrap_err();
        match err {
            SearchEngineError::Unavailable { detail, .. } => {
                assert!(detail.contains("all engines cooling down or over budget"));
            }
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn wait_loop_breaks_once_engine_becomes_eligible() {
        // Brave and Google are cooled down for an hour; Bing only needs its
        // 1s minimum interval to elapse. The loop must wait it out (~1s,
        // sleeping the full remaining interval) and then dispatch to Bing.
        let state = Mutex::new({
            let mut state = cold_state();
            let future = Instant::now() + Duration::from_secs(3600);
            state.cooldowns.insert(SearchEngine::Brave, future);
            state.cooldowns.insert(SearchEngine::Google, future);
            state
        });
        let pacing = Mutex::new(cold_pacing());
        pacing.lock().unwrap().record(SearchEngine::Bing, unix_now_ms());
        let notify = Notify::new();
        let engine = wait_for_available_engine(
            &state,
            &pacing,
            &notify,
            Instant::now() + MAX_AUTO_WAIT,
            SearchEngineError::Unavailable {
                engine: SearchEngine::Brave,
                detail: "all engines cooling down or over budget".to_string(),
            },
        )
        .await
        .expect("Bing must become eligible after its 1s interval");
        assert_eq!(engine, SearchEngine::Bing);
    }

    #[test]
    fn pick_and_reserve_stamps_engine_atomically() {
        // Cold state: the pick must hand out the priority engine and, inside
        // the same lock scope, stamp its persisted last-call so it becomes
        // ineligible under the pacing rules.
        let state = Mutex::new(cold_state());
        let pacing = Mutex::new(cold_pacing());
        let engine = pick_and_reserve(&state, &pacing).expect("cold state: Brave eligible");
        assert_eq!(engine, SearchEngine::Brave);
        let now_ms = unix_now_ms();
        let last_call = pacing
            .lock()
            .unwrap()
            .last_call_ms(engine)
            .expect("pick_and_reserve must stamp the picked engine");
        assert!(
            !pacing_ready(Some(last_call), min_interval(engine), now_ms),
            "the reservation stamp must make the engine ineligible again"
        );
    }

    #[tokio::test]
    async fn two_concurrent_picks_get_distinct_engines() {
        // The t=0 regression: two parallel searches both call
        // pick_and_reserve on a cold store. The reservation must be visible
        // to the second pick (Brave's 60s interval is nowhere near elapsed),
        // so the two picks must land on different engines — a pre-fix race
        // would hand both tasks Brave and double-dispatch the server.
        let state = Arc::new(Mutex::new(cold_state()));
        let pacing = Arc::new(Mutex::new(cold_pacing()));
        let (s1, p1) = (state.clone(), pacing.clone());
        let (s2, p2) = (state.clone(), pacing.clone());
        let (a, b) = tokio::join!(
            tokio::task::spawn_blocking(move || pick_and_reserve(&s1, &p1)),
            tokio::task::spawn_blocking(move || pick_and_reserve(&s2, &p2)),
        );
        let a = a.expect("first pick task joined").expect("first pick");
        let b = b.expect("second pick task joined").expect("second pick");
        assert_ne!(
            a, b,
            "the second pick must see the first pick's reservation"
        );
    }

    #[tokio::test]
    async fn wait_loop_poll_reserves_selected_engine() {
        // Brave and Google are cooled down for an hour; Bing's 1s interval
        // has already elapsed, so the first poll picks it. The pick must
        // stamp the store (reserve) at pick time, not merely report it.
        let state = Mutex::new({
            let mut state = cold_state();
            let future = Instant::now() + Duration::from_secs(3600);
            state.cooldowns.insert(SearchEngine::Brave, future);
            state.cooldowns.insert(SearchEngine::Google, future);
            state
        });
        let pacing = Mutex::new(cold_pacing());
        pacing
            .lock()
            .unwrap()
            .record(SearchEngine::Bing, unix_now_ms() - 2000);
        let notify = Notify::new();
        let engine = wait_for_available_engine(
            &state,
            &pacing,
            &notify,
            Instant::now() + MAX_AUTO_WAIT,
            SearchEngineError::Unavailable {
                engine: SearchEngine::Brave,
                detail: "all engines cooling down or over budget".to_string(),
            },
        )
        .await
        .expect("Bing's interval has elapsed: must be picked immediately");
        assert_eq!(engine, SearchEngine::Bing);
        // The reservation happened under the pick's lock scope: the store
        // now holds a fresh stamp that makes Bing ineligible again.
        let now_ms = unix_now_ms();
        let last_call = pacing
            .lock()
            .unwrap()
            .last_call_ms(engine)
            .expect("wait-loop pick must stamp the store");
        assert!(
            !pacing_ready(Some(last_call), min_interval(engine), now_ms),
            "Bing must be reserved (ineligible) right after the wait loop returns"
        );
    }

    #[tokio::test]
    async fn wait_loop_sleeps_full_remaining_not_poll_step() {
        // Bing's 1s interval just started; Brave and Google are cooled down
        // for an hour. The wait loop must sleep the full ~1s remaining (not a
        // 500ms poll step) before Bing becomes eligible.
        let state = Mutex::new({
            let mut state = cold_state();
            let future = Instant::now() + Duration::from_secs(3600);
            state.cooldowns.insert(SearchEngine::Brave, future);
            state.cooldowns.insert(SearchEngine::Google, future);
            state
        });
        let pacing = Mutex::new(cold_pacing());
        pacing.lock().unwrap().record(SearchEngine::Bing, unix_now_ms());
        let notify = Notify::new();
        let start = Instant::now();
        let engine = wait_for_available_engine(
            &state,
            &pacing,
            &notify,
            Instant::now() + MAX_AUTO_WAIT,
            SearchEngineError::Unavailable {
                engine: SearchEngine::Brave,
                detail: "all engines cooling down or over budget".to_string(),
            },
        )
        .await
        .expect("Bing must become eligible after its 1s interval");
        assert_eq!(engine, SearchEngine::Bing);
        let elapsed = start.elapsed();
        assert!(
            elapsed >= Duration::from_millis(900),
            "must sleep the full remaining interval, got {elapsed:?}"
        );
    }

    #[test]
    fn record_dispatch_outcome_stamps_pacing_on_success() {
        // Auto and pin modes share record_dispatch_outcome; a success must
        // stamp the persisted last-call so the next dispatch is paced.
        let router = SearchRouter::with_pacing(None, Arc::new(Mutex::new(cold_pacing())));
        let result: Result<Vec<EngineSearchResult>, SearchEngineError> = Ok(vec![]);
        router.record_dispatch_outcome(SearchEngine::Bing, &result);
        let pacing = router.pacing.lock().unwrap();
        assert!(
            pacing.last_call_ms(SearchEngine::Bing).is_some(),
            "success must stamp the last-call timestamp"
        );
        assert!(
            pacing.cooldown_until_ms(SearchEngine::Bing).is_none(),
            "success must not set a cooldown"
        );
    }

    #[test]
    fn record_dispatch_outcome_records_cooldown_on_rate_limit() {
        // A rate-limit outcome must set the persisted cooldown AND stamp the
        // last-call under a single lock acquisition.
        let router = SearchRouter::with_pacing(None, Arc::new(Mutex::new(cold_pacing())));
        let result: Result<Vec<EngineSearchResult>, SearchEngineError> =
            Err(SearchEngineError::RateLimited {
                engine: SearchEngine::Brave,
                detail: "HTTP 429".to_string(),
            });
        router.record_dispatch_outcome(SearchEngine::Brave, &result);
        let pacing = router.pacing.lock().unwrap();
        assert!(
            pacing.cooldown_until_ms(SearchEngine::Brave).is_some(),
            "rate limit must set a persisted cooldown"
        );
        assert!(
            pacing.last_call_ms(SearchEngine::Brave).is_some(),
            "rate limit must also stamp the last-call timestamp"
        );
    }

    #[test]
    fn auto_mode_falls_back_when_first_engine_returns_empty() {
        let mut accum = FallbackAccum::default();
        // First engine answers with a legitimate empty set: auto mode must
        // NOT serve it — the fallback loop keeps going to the next engine.
        assert!(accum.record(Ok(vec![])).is_none());
        assert!(accum.saw_empty_ok);
        // Second engine returns results: those are served immediately.
        let served = accum
            .record(Ok(vec![engine_result(
                "Hit",
                "https://example.com/hit",
                "snippet",
            )]))
            .expect("non-empty results must be served");
        assert_eq!(served.len(), 1);
        assert_eq!(served[0].title, "Hit");
    }

    #[test]
    fn auto_mode_all_engines_empty_yields_empty_ok() {
        let mut accum = FallbackAccum::default();
        for _ in SearchEngine::PRIORITY {
            assert!(
                accum.record(Ok(vec![])).is_none(),
                "empty Ok must never end the auto fallback loop"
            );
        }
        assert!(accum.errors.is_empty());
        let verdict = accum
            .exhausted()
            .expect("no engine failed; the query legitimately has zero results");
        assert!(verdict.is_empty());
    }

    #[test]
    fn auto_mode_exhaustion_with_errors_yields_all_engines_failed() {
        let mut accum = FallbackAccum::default();
        assert!(accum
            .record(Err(SearchEngineError::RateLimited {
                engine: SearchEngine::Brave,
                detail: "HTTP 429".to_string(),
            }))
            .is_none());
        match accum.exhausted().unwrap_err() {
            SearchEngineError::AllEnginesFailed(errors) => assert_eq!(errors.len(), 1),
            other => panic!("expected AllEnginesFailed, got {other:?}"),
        }
    }

    #[test]
    fn seeded_cooldown_makes_auto_skip_engine() {
        // A cooldown persisted by a previous invocation (e.g. a rate-limit)
        // is seeded into the in-memory map on router construction; auto mode
        // must skip the engine until it expires.
        let now = Instant::now();
        let now_ms = unix_now_ms();
        let mut pacing = cold_pacing();
        pacing.record_cooldown(SearchEngine::Brave, now_ms + 60_000);
        let mut state = cold_state();
        state.cooldowns = seed_cooldowns(&pacing, now, now_ms);
        assert_eq!(
            next_engine(&state, &pacing, &EngineChoice::Auto).unwrap(),
            SearchEngine::Bing
        );
    }

    #[test]
    fn seeded_cooldown_expired_is_dropped() {
        // An expired persisted cooldown must not block the engine.
        let now = Instant::now();
        let now_ms = unix_now_ms();
        let mut pacing = cold_pacing();
        pacing.record_cooldown(SearchEngine::Brave, now_ms - 1000);
        let mut state = cold_state();
        state.cooldowns = seed_cooldowns(&pacing, now, now_ms);
        assert!(state.cooldowns.is_empty());
        assert_eq!(
            next_engine(&state, &pacing, &EngineChoice::Auto).unwrap(),
            SearchEngine::Brave
        );
    }

    #[test]
    fn rate_limit_outcome_reports_cooldown_to_persist() {
        let mut state = cold_state();
        let error = Err(SearchEngineError::RateLimited {
            engine: SearchEngine::Brave,
            detail: "HTTP 429".to_string(),
        });
        let (engine, until_ms) = record_outcome(&mut state, SearchEngine::Brave, &error)
            .expect("rate limit must report a persisted cooldown");
        assert_eq!(engine, SearchEngine::Brave);
        assert!(until_ms > unix_now_ms(), "cooldown until-ms must be in the future");
        // In-memory cooldown still set (existing behavior unchanged).
        assert_eq!(
            next_engine(&state, &cold_pacing(), &EngineChoice::Auto).unwrap(),
            SearchEngine::Bing
        );
    }

    #[test]
    fn success_outcome_reports_no_cooldown() {
        let mut state = cold_state();
        assert!(record_outcome(&mut state, SearchEngine::Bing, &Ok(vec![])).is_none());
        // No cooldown entry appeared in state (the query budget is stamped
        // into the pacing store by the caller instead).
        assert!(!state.cooldowns.contains_key(&SearchEngine::Bing));
    }

    #[test]
    fn translate_wrapper_url_detection() {
        assert!(is_translate_wrapper_url(
            "https://example-com.translate.goog/_x_tr_sl=en&_x_tr_tl=zh-CN"
        ));
        assert!(is_translate_wrapper_url(
            "https://translate.google.com/translate?sl=auto&tl=zh-CN&u=example.com"
        ));
        assert!(is_translate_wrapper_url(
            "https://www.google.com/url?q=https://example.com/doc&sa=U"
        ));
        assert!(!is_translate_wrapper_url("https://example.com/doc"));
        assert!(!is_translate_wrapper_url(""));
        // Structure-based matching: a bare "/url?q=" substring elsewhere must
        // not false-positive.
        assert!(!is_translate_wrapper_url(
            "https://example.com/url?q=not-a-wrapper"
        ));
        assert!(!is_translate_wrapper_url(
            "https://translate.example.com/translate?u=example.org"
        ));
        assert!(!is_translate_wrapper_url(
            "https://www.google.com/search?q=url%3Fq%3Dtest"
        ));
    }

    #[test]
    fn non_latin_script_detection() {
        // CJK Unified Ideographs, Hiragana, Katakana, and Hangul all count.
        assert!(has_non_latin_script("中国 - 百度百科"));
        assert!(has_non_latin_script("ひらがなのページ"));
        assert!(has_non_latin_script("カタカナのページ"));
        assert!(has_non_latin_script("한국어 위키백과"));
        // Vietnamese (Latin Extended Additional) and Cyrillic junk.
        assert!(has_non_latin_script("Hiểu về Index"));
        assert!(has_non_latin_script("Понимание индекса"));
        assert!(has_non_latin_script("Ελληνική σελίδα"));
        assert!(has_non_latin_script("صفحة عربية"));
        assert!(has_non_latin_script("หน้าไทย"));
        assert!(has_non_latin_script("हिन्दी पृष्ठ"));
        // ASCII / Latin-script titles pass.
        assert!(!has_non_latin_script("Rust Programming Language"));
        assert!(!has_non_latin_script("Cafe - naive uber"));
        assert!(!has_non_latin_script(""));
    }

    #[test]
    fn map_engine_results_drops_translate_wrappers() {
        let results = vec![
            engine_result(
                "Translated",
                "https://example-com.translate.goog/page",
                "wrapper snippet",
            ),
            engine_result(
                "Proxied",
                "https://translate.google.com/translate?u=https://example.org",
                "proxy snippet",
            ),
            engine_result(
                "Redirected",
                "https://www.google.com/url?q=https://example.net/doc&sa=U",
                "redirect snippet",
            ),
            engine_result("Kept", "https://example.com/real", "real snippet"),
        ];
        let mapped = map_engine_results(results, "https://example.com/search?q=test", 7);
        assert_eq!(mapped.len(), 1, "all translate/redirect wrappers filtered");
        assert_eq!(mapped[0].title, "Kept");
    }

    #[test]
    fn map_engine_results_drops_non_latin_titles() {
        let results = vec![
            engine_result("中国 百度百科", "https://baike.baidu.com/item/x", "中文结果"),
            engine_result("한국어 위키백과", "https://ko.wikipedia.org/wiki/러스트", "한국어"),
            engine_result(
                "Rust (programming language) - Wikipedia",
                "https://en.wikipedia.org/wiki/Rust",
                "English result",
            ),
        ];
        let mapped = map_engine_results(results, "https://example.com/search?q=test", 7);
        assert_eq!(mapped.len(), 1, "localized (non-Latin) titles dropped");
        assert_eq!(
            mapped[0].title,
            "Rust (programming language) - Wikipedia"
        );
        assert_eq!(mapped[0].position, 1, "positions renumbered after filtering");
    }

    #[test]
    fn map_engine_results_drops_non_latin_snippets() {
        // An English title with a non-English (Chinese) snippet must be
        // rejected — the snippet-level filter closes the blind spot where
        // only titles were checked.
        let results = vec![
            engine_result(
                "Rust programming language",
                "https://example.com/rust",
                "Rust 是一种系统编程语言",
            ),
            engine_result(
                "Cyrillic snippet",
                "https://example.org/cyr",
                "Понимание индекса",
            ),
            engine_result(
                "Vietnamese snippet",
                "https://example.net/vn",
                "Hiểu về Index",
            ),
            engine_result(
                "Kept",
                "https://example.com/kept",
                "A fully English snippet about Rust.",
            ),
        ];
        let mapped = map_engine_results(results, "https://example.com/search?q=test", 7);
        assert_eq!(mapped.len(), 1, "non-Latin snippets dropped even with English titles");
        assert_eq!(mapped[0].title, "Kept");
    }

    #[test]
    fn dictionary_junk_detection() {
        // Known dictionary/definition domains (exact host and subdomain).
        assert!(is_dictionary_junk(
            "https://dictionary.cambridge.org/dictionary/english/rust",
            "Rust",
            "a reddish-brown substance",
        ));
        assert!(is_dictionary_junk(
            "https://www.merriam-webster.com/dictionary/rust",
            "Rust",
            "the reddish brittle coating",
        ));
        assert!(is_dictionary_junk(
            "https://www.dictionary.com/browse/rust",
            "Rust",
            "the red or orange coating",
        ));
        assert!(is_dictionary_junk(
            "https://www.scribbr.com/definitions/rust/",
            "Rust",
            "definition of rust",
        ));
        assert!(is_dictionary_junk(
            "https://www.thefreedictionary.com/rust",
            "Rust",
            "any of various metallic coatings",
        ));
        assert!(is_dictionary_junk(
            "https://www.vocabulary.com/dictionary/rust",
            "Rust",
            "a red or brown oxide coating",
        ));
        assert!(is_dictionary_junk(
            "https://www.collinsdictionary.com/dictionary/english/rust",
            "Rust",
            "a reddish-brown oxide coating",
        ));
        assert!(is_dictionary_junk(
            "https://www.oxfordlearnersdictionaries.com/definition/english/rust",
            "Rust",
            "a reddish-brown substance",
        ));
        // Title is a single word + "definition".
        assert!(is_dictionary_junk(
            "https://example.com/rust",
            "Rust definition",
            "some snippet",
        ));
        // Snippet contains "definition of".
        assert!(is_dictionary_junk(
            "https://example.com/rust",
            "Rust",
            "the definition of rust is a reddish coating",
        ));
        // Legitimate content is NOT over-filtered.
        assert!(!is_dictionary_junk(
            "https://en.wikipedia.org/wiki/Rust",
            "Rust (programming language) - Wikipedia",
            "Rust is a multi-paradigm programming language.",
        ));
        assert!(!is_dictionary_junk(
            "https://example.com/rust",
            "Rust programming language",
            "A systems language focused on safety.",
        ));
        assert!(!is_dictionary_junk(
            "https://example.com/rust",
            "Rust programming language definition",
            "A multi-word title mentioning definition is not a dictionary page.",
        ));
        assert!(!is_dictionary_junk("", "", ""));
    }

    #[test]
    fn map_engine_results_drops_dictionary_junk() {
        let results = vec![
            engine_result(
                "Rust",
                "https://dictionary.cambridge.org/dictionary/english/rust",
                "a reddish-brown substance",
            ),
            engine_result(
                "Rust definition",
                "https://example.com/rust-def",
                "single-word title + definition",
            ),
            engine_result(
                "Rust",
                "https://example.org/rust",
                "the definition of rust is a coating",
            ),
            engine_result(
                "Rust (programming language) - Wikipedia",
                "https://en.wikipedia.org/wiki/Rust",
                "Rust is a multi-paradigm programming language.",
            ),
        ];
        let mapped = map_engine_results(results, "https://example.com/search?q=test", 7);
        assert_eq!(mapped.len(), 1, "dictionary/definition pages filtered");
        assert_eq!(mapped[0].title, "Rust (programming language) - Wikipedia");
    }

    #[test]
    fn map_engine_results_filters_dedups_and_renumbers() {
        let results = vec![
            engine_result(
                "Junk",
                "https://accounts.google.com/signin",
                "junk result",
            ),
            engine_result("Frag", "https://example.com/doc#:~:text=hi", "fragment link"),
            engine_result("Empty", "https://example.org/empty", "   "),
            engine_result("Dup A", "https://example.com/doc#section1", "same base"),
            engine_result("Dup B", "https://example.com/doc#section2", "same base again"),
            engine_result("  Kept  ", "https://en.wikipedia.org/kept", "kept snippet"),
        ];

        let mapped = map_engine_results(results, "https://example.com/search?q=test", 42);

        assert_eq!(mapped.len(), 2, "junk, fragment, empty snippet, and dup filtered");
        // First base-URL occurrence wins; positions are renumbered 1-based.
        assert_eq!(mapped[0].title, "Dup A");
        assert_eq!(mapped[0].url, "https://example.com/doc#section1");
        assert_eq!(mapped[0].position, 1);
        assert_eq!(mapped[1].title, "Kept", "title trimmed");
        assert_eq!(mapped[1].position, 2);

        // Provenance: source_url is the result URL, method is Search.
        assert_eq!(mapped[1].provenance.source_url, "https://en.wikipedia.org/kept");
        assert_eq!(mapped[1].provenance.method, ExtractionMethod::Search);
        assert_eq!(mapped[1].provenance.agent, GTHINGS_AGENT);
        assert_eq!(mapped[1].provenance.duration_ms, 42);

        // Domain authority rounded to two decimals, kept as f64 (no float
        // artifacts), and a 0.9-tier domain (wikipedia.org) stays 0.9.
        assert_eq!(mapped[1].domain_authority, 0.9);
        assert_eq!(mapped[1].domain_authority.to_string(), "0.9");
    }

    #[test]
    fn map_engine_results_dedups_normalized_url_and_title() {
        let results = vec![
            engine_result("Same", "https://www.Example.com/Path/", "first"),
            engine_result("Same", "https://example.com/Path", "second"),
            engine_result("Same", "https://example.com/Path?utm_source=x", "third"),
            engine_result("  Duplicate   Title ", "https://a.example/one", "title a"),
            engine_result("duplicate title", "https://b.example/two", "title b"),
            engine_result("Unique", "https://c.example/three", "unique"),
        ];
        let mapped = map_engine_results(results, "https://example.com/search?q=test", 7);
        // www/non-www + trailing slash + tracking query collapse to one base;
        // the two duplicate-title results collapse to one.
        assert_eq!(mapped.len(), 3, "www, trailing slash, tracking, and dup titles deduped");
        assert_eq!(mapped[0].title, "Same");
        assert_eq!(mapped[1].title, "Duplicate Title", "title trimmed + collapsed");
        assert_eq!(mapped[2].title, "Unique");
    }

    #[test]
    fn map_engine_results_filters_empty_titles() {
        let results = vec![
            engine_result("   ", "https://example.com/blank", "non-empty snippet"),
            engine_result("", "https://example.org/empty", "also non-empty"),
            engine_result("Kept", "https://example.net/kept", "kept snippet"),
        ];
        let mapped = map_engine_results(results, "https://example.com/search?q=test", 7);
        assert_eq!(mapped.len(), 1, "empty titles filtered even with a snippet");
        assert_eq!(mapped[0].title, "Kept");
    }

    #[test]
    fn map_engine_results_populates_source_type() {
        let results = vec![
            engine_result("Repo", "https://github.com/rust-lang/rust", "github"),
            engine_result("Paper", "https://arxiv.org/abs/2301.00001", "arxiv"),
            engine_result("Doc", "https://example.com/guide.pdf", "pdf"),
            engine_result("Web", "https://example.com/page", "web"),
        ];
        let mapped = map_engine_results(results, "https://example.com/search?q=test", 7);
        assert_eq!(mapped.len(), 4);
        assert_eq!(mapped[0].source_type, "github");
        assert_eq!(mapped[1].source_type, "paper");
        assert_eq!(mapped[2].source_type, "pdf");
        assert_eq!(mapped[3].source_type, "web");
    }

    #[test]
    fn effective_query_rewrites_per_engine() {
        let q = "(docker OR podman) compose AROUND(3)";
        // Bing does not support parens or AROUND(n): both are stripped, the
        // rest kept in order.
        assert_eq!(effective_query(SearchEngine::Bing, q), "docker OR podman compose");
        // Google supports parens and AROUND(n): untouched.
        assert_eq!(effective_query(SearchEngine::Google, q), q);
    }

    #[test]
    fn effective_query_passthrough_without_operators() {
        let q = "redis streams";
        for engine in [SearchEngine::Brave, SearchEngine::Bing, SearchEngine::Google] {
            assert_eq!(effective_query(engine, q), q, "no-operator query must be unchanged");
        }
    }

    #[tokio::test]
    async fn wait_loop_wakes_on_notify() {
        // Event-driven wait: Bing's 1s interval just started, so the loop
        // would otherwise sleep the full ~1s. A concurrent dispatch that
        // records pacing (making Bing eligible) and calls notify_waiters must
        // wake the loop immediately — well before the interval elapses.
        let state = Mutex::new({
            let mut state = cold_state();
            let future = Instant::now() + Duration::from_secs(3600);
            state.cooldowns.insert(SearchEngine::Brave, future);
            state.cooldowns.insert(SearchEngine::Google, future);
            state
        });
        let pacing = Arc::new(Mutex::new(cold_pacing()));
        pacing.lock().unwrap().record(SearchEngine::Bing, unix_now_ms());
        let notify = Arc::new(Notify::new());

        // A sibling task "dispatches" Bing after 100ms: stamps a fresh
        // last-call (making it eligible) and wakes the waiter.
        let (p2, n2) = (pacing.clone(), notify.clone());
        let waker = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            p2.lock().unwrap().record(SearchEngine::Bing, unix_now_ms() - 2000);
            n2.notify_waiters();
        });

        let start = Instant::now();
        let engine = wait_for_available_engine(
            &state,
            &pacing,
            &notify,
            Instant::now() + MAX_AUTO_WAIT,
            SearchEngineError::Unavailable {
                engine: SearchEngine::Brave,
                detail: "all engines cooling down or over budget".to_string(),
            },
        )
        .await
        .expect("notify must wake the wait loop to Bing");
        waker.await.expect("waker task joined");
        assert_eq!(engine, SearchEngine::Bing);
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_millis(500),
            "must wake on notify (~100ms), not sleep the full 1s interval, got {elapsed:?}"
        );
    }

    #[test]
    fn shared_pacing_across_routers() {
        // Two routers built with the same injected store must observe each
        // other's pacing stamps — the shared-pacing contract.
        let pacing = Arc::new(Mutex::new(cold_pacing()));
        let r1 = SearchRouter::with_pacing(None, pacing.clone());
        let r2 = SearchRouter::with_pacing(None, pacing.clone());
        r1.record_pacing(SearchEngine::Bing);
        assert!(
            r2.pacing.lock().unwrap().last_call_ms(SearchEngine::Bing).is_some(),
            "router 2 must observe router 1's pacing stamp via the shared store"
        );
    }

    #[test]
    fn new_routers_share_global_pacing() {
        // Routers built via SearchRouter::new must share the process-wide
        // global store, so pacing persists across router instances.
        let r1 = SearchRouter::new(None);
        let r2 = SearchRouter::new(None);
        r1.record_pacing(SearchEngine::Bing);
        assert!(
            r2.pacing.lock().unwrap().last_call_ms(SearchEngine::Bing).is_some(),
            "routers built via new() must share the global pacing store"
        );
    }
}
