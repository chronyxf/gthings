//! Engine selection, pacing, cooldowns, and auto-mode fallback accumulation.
//!
//! Pure state-machine helpers (no I/O) shared by the dispatch path
//! ([`super::dispatch`]) and the router constructor ([`super::RouterState`]).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::sync::Notify;

use crate::engine::pacing::PacingStore;
use crate::engine::{EngineChoice, EngineSearchResult, SearchEngine, SearchEngineError};

use super::RouterState;
use super::{CAPTCHA_COOLDOWN, RATE_LIMITED_COOLDOWN, WAIT_POLL};

/// Minimum interval between queries for `engine` (the token-bucket refill).
///
/// The interval table lives in [`crate::engine::pacing::min_interval_ms`] (the
/// pacing store owns last-call timestamps and exposes `pacing_snapshot()` for
/// healthz, which needs the same budgets), so there is a single source of
/// truth for query budgets.
pub(crate) fn min_interval(engine: SearchEngine) -> Duration {
    Duration::from_millis(crate::engine::pacing::min_interval_ms(engine))
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
/// first engine in `priority` order that is neither cooling down nor over
/// budget **and** whose persisted minimum interval has elapsed; with
/// [`EngineChoice::Pin`] returns the pinned engine unconditionally (the
/// caller is responsible for enforcing cooldown and pacing).
pub(crate) fn next_engine(
    state: &RouterState,
    pacing: &PacingStore,
    choice: &EngineChoice,
    priority: &[SearchEngine],
) -> Result<SearchEngine, SearchEngineError> {
    match choice {
        EngineChoice::Pin(engine) => Ok(*engine),
        EngineChoice::Auto => {
            let now = Instant::now();
            let now_ms = unix_now_ms();
            for &engine in priority {
                if engine_ready(state, engine, now)
                    && pacing_ready(pacing.last_call_ms(engine), min_interval(engine), now_ms)
                {
                    return Ok(engine);
                }
            }
            Err(SearchEngineError::Unavailable {
                engine: priority[0],
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
///
/// When the picked engine is a paid backend and the aggregate API quota is
/// exhausted, [`SearchEngineError::QuotaExceeded`] is returned (mapped to
/// HTTP 429 `quota-exceeded` by consumers) instead of reserving it — quota
/// spend is only counted once a dispatch is actually reserved.
pub(crate) fn pick_and_reserve(
    state: &Mutex<RouterState>,
    pacing: &Mutex<PacingStore>,
    priority: &'static [SearchEngine],
) -> Result<SearchEngine, SearchEngineError> {
    let state = state.lock().unwrap_or_else(|e| e.into_inner());
    let mut pacing = pacing.lock().unwrap_or_else(|e| e.into_inner());
    let engine = next_engine(&state, &pacing, &EngineChoice::Auto, priority)?;
    if engine.is_paid() && pacing.quota_exceeded() {
        return Err(SearchEngineError::QuotaExceeded {
            engine,
            detail: format!(
                "aggregate API quota exhausted (spend {}, limit {})",
                pacing.quota_spend(),
                pacing.quota_limit()
            ),
        });
    }
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
pub(crate) async fn wait_for_available_engine(
    state: &Mutex<RouterState>,
    pacing: &Mutex<PacingStore>,
    notify: &Notify,
    deadline: Instant,
    unavailable: SearchEngineError,
    priority: &'static [SearchEngine],
) -> Result<SearchEngine, SearchEngineError> {
    loop {
        // Every pass atomically picks **and reserves** the first eligible
        // engine (pick + stamp under one lock scope). When two tasks race,
        // the loser's next poll sees the winner's reservation and picks a
        // different engine — or keeps waiting.
        match pick_and_reserve(state, pacing, priority) {
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
                        priority,
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
pub(crate) fn unix_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Remaining milliseconds until `engine`'s minimum interval has elapsed since
/// its last recorded call, or `None` when it may be dispatched right away.
///
/// Pure decision helper shared by auto mode (skip) and pin mode (wait);
/// delegates to the single implementation in the pacing store
/// ([`crate::engine::pacing::remaining_ms`]). `last_call_ms` is the persisted
/// last-call timestamp, `min_interval_ms` the engine's minimum interval,
/// `now_ms` the current unix millis.
pub(crate) fn pacing_remaining_ms(
    last_call_ms: Option<u64>,
    min_interval_ms: u64,
    now_ms: u64,
) -> Option<u64> {
    crate::engine::pacing::remaining_ms(last_call_ms, min_interval_ms, now_ms)
}

/// Whether `engine` may be dispatched under the persisted pacing rules.
pub(crate) fn pacing_ready(last_call_ms: Option<u64>, min_interval: Duration, now_ms: u64) -> bool {
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
pub(crate) fn earliest_available_remaining_ms(
    state: &RouterState,
    pacing: &PacingStore,
    now: Instant,
    now_ms: u64,
    priority: &[SearchEngine],
) -> Option<u64> {
    let mut earliest: Option<u64> = None;
    for &engine in priority {
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
pub(crate) fn record_outcome(
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
pub(crate) fn seed_cooldowns(
    pacing: &PacingStore,
    now: Instant,
    now_ms: u64,
) -> HashMap<SearchEngine, Instant> {
    let mut cooldowns = HashMap::new();
    for engine in SearchEngine::HYBRID_PRIORITY {
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
/// As the fallback loop walks the engine priority list, this tracks engine
/// failures and legitimate empty result sets. An engine answering `Ok` with
/// an empty vector is a *healthy* answer ("zero results for this query on
/// this engine") and must not mask the remaining engines — the loop keeps
/// going. Only when every engine has been tried without a single failure
/// does the query genuinely have zero results.
#[derive(Debug, Default)]
pub(crate) struct FallbackAccum {
    /// Errors from failed engines; non-empty ⇒ `AllEnginesFailed` verdict.
    pub(crate) errors: Vec<SearchEngineError>,
    /// Whether any engine returned a legitimate empty result set.
    pub(crate) saw_empty_ok: bool,
}

impl FallbackAccum {
    /// Record one engine outcome: `Some(results)` serves non-empty results
    /// immediately; `None` keeps the fallback loop going (empty `Ok` or
    /// `Err`).
    pub(crate) fn record(
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
    pub(crate) fn exhausted(self) -> Result<Vec<EngineSearchResult>, SearchEngineError> {
        if !self.errors.is_empty() {
            Err(SearchEngineError::AllEnginesFailed(self.errors))
        } else {
            Ok(Vec::new())
        }
    }
}
