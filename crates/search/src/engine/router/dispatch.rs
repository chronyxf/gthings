//! Dispatch, fallback search, outcome recording, and the dispatch observer.
//!
//! The [`SearchRouter`] dispatch path lives here (the struct and its
//! constructors live in [`super`]): per-engine dispatch, auto/pin search with
//! fallback, the shared outcome funnel ([`record_dispatch_outcome`]), and the
//! classified [`DispatchOutcome`] surfaced to streaming consumers.

#[cfg(test)]
use std::sync::Arc;
use std::time::{Duration, Instant};

#[cfg(test)]
use std::future::Future;
#[cfg(test)]
use std::pin::Pin;

use crate::engine::technique;
use crate::engine::{
    EngineChoice, EngineMode, EngineSearchResult, SearchEngine, SearchEngineBackend,
    SearchEngineError, SearchOptions,
};

use super::select::{
    FallbackAccum, min_interval, pacing_remaining_ms, pick_and_reserve, record_outcome,
    unix_now_ms, wait_for_available_engine,
};
use super::{MAX_AUTO_WAIT, SearchRouter};

/// Classified outcome of one engine dispatch, surfaced to streaming consumers
/// through the per-call dispatch observer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchOutcome {
    /// The engine served non-empty results (emitted as `Result` events
    /// post-map by the streaming projection).
    Served,
    /// The engine served nothing (empty `Ok` or a non-block error); auto mode
    /// falls back to the next engine.
    Fallback,
    /// The engine was rate-limited (HTTP 429) and entered a 5-minute cooldown.
    RateLimited,
    /// The engine served a captcha/block page and entered a 30-minute cooldown.
    Captcha,
    /// The engine just entered a cooldown block (after a rate-limit/captcha).
    Cooldown,
}

/// Per-call dispatch observer: invoked once per engine dispatch from the
/// shared outcome funnel ([`SearchRouter::record_dispatch_outcome`]).
///
/// Threaded as a parameter — never stored on the router — so concurrent
/// searches on a shared router (e.g. the batch path) can never clobber each
/// other's observer. Streaming consumers map each [`DispatchOutcome`] to a
/// [`SearchEvent`](crate::stream::SearchEvent).
pub(crate) type DispatchObserver<'a> = &'a (dyn Fn(SearchEngine, DispatchOutcome) + Sync);

/// The query actually sent to `engine`'s backend: the raw query rewritten
/// per-engine by [`technique::rewrite`], stripping operators the engine does
/// not support. No-operator queries come back byte-identical.
pub(crate) fn effective_query(engine: SearchEngine, query: &str) -> String {
    technique::rewrite(query, engine).into_owned()
}

/// Test-only dispatch seam: a boxed `search(query, count)` future with the
/// same contract as [`SearchEngineBackend::search`], but dyn-compatible — the
/// real trait's `async fn` cannot be used as `dyn`. Shared by reference (never
/// cloned); each dispatch borrows it for the duration of the search.
#[cfg(test)]
pub(crate) type BoxedSearch = Arc<
    dyn for<'a> Fn(
            &'a str,
            usize,
            &'a SearchOptions,
        ) -> Pin<
            Box<
                dyn Future<Output = Result<Vec<EngineSearchResult>, SearchEngineError>> + Send + 'a,
            >,
        > + Send
        + Sync,
>;

impl SearchRouter {
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
        options: &SearchOptions,
    ) -> Result<Vec<EngineSearchResult>, SearchEngineError> {
        let effective_query = effective_query(engine, query);
        #[cfg(test)]
        if let Some(fake) = &self.fake_backend {
            return fake(&effective_query, count, options).await;
        }
        match engine {
            SearchEngine::Brave => match &self.brave {
                Some(brave) => brave.search(&effective_query, count, options).await,
                None => Err(SearchEngineError::Unavailable {
                    engine,
                    detail: "no browser session; Brave backend not constructed".to_string(),
                }),
            },
            SearchEngine::Bing => self.bing.search(&effective_query, count, options).await,
            SearchEngine::Google => match &self.google {
                Some(google) => google.search(&effective_query, count, options).await,
                None => Err(SearchEngineError::Unavailable {
                    engine,
                    detail: "no browser session; Google backend not constructed".to_string(),
                }),
            },
            SearchEngine::BraveApi => {
                self.brave_api
                    .search(&effective_query, count, options)
                    .await
            }
            SearchEngine::Tavily => self.tavily.search(&effective_query, count, options).await,
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
        options: &SearchOptions,
    ) -> Result<Vec<EngineSearchResult>, SearchEngineError> {
        self.search_with_fallback_observed(query, count, choice, options, None)
            .await
    }

    /// [`Self::search_with_fallback`] with a per-call dispatch observer.
    ///
    /// Streaming consumers (see [`crate::search_streaming`]) pass an observer
    /// here so engine lifecycle events are surfaced from the shared outcome
    /// funnel. The observer is a parameter, never router state, so concurrent
    /// searches on one router cannot clobber each other's observer.
    pub(crate) async fn search_with_fallback_observed(
        &self,
        query: &str,
        count: usize,
        choice: EngineChoice,
        options: &SearchOptions,
        observe: Option<DispatchObserver<'_>>,
    ) -> Result<Vec<EngineSearchResult>, SearchEngineError> {
        match choice {
            EngineChoice::Pin(engine) => {
                self.search_pinned(query, count, engine, options, observe)
                    .await
            }
            EngineChoice::Auto => self.search_auto(query, count, options, observe).await,
        }
    }

    /// Auto mode: priority-order fallback across all available engines.
    ///
    /// Hybrid mode runs in two phases: the free engines (Google, Brave, Bing)
    /// are tried first with no paid spend; when the free
    /// phase yields nothing — every engine empty, failed, or blocked into
    /// cooldown (429/captcha) — the search falls back to the paid API
    /// backends (Brave API, Tavily), gated by the aggregate quota. `free` and
    /// `api` modes run exactly one phase (their [`EngineMode::priority`]
    /// list).
    ///
    /// An engine answering `Ok` with an empty vector is a healthy zero-result
    /// answer; it does not end the search (it could mask a still-healthy
    /// engine), so the loop keeps going. If every engine answers empty and
    /// none failed, the query genuinely has zero results.
    async fn search_auto(
        &self,
        query: &str,
        count: usize,
        options: &SearchOptions,
        observe: Option<DispatchObserver<'_>>,
    ) -> Result<Vec<EngineSearchResult>, SearchEngineError> {
        let mut accum = FallbackAccum::default();
        match self.mode {
            // Single-phase modes: free-only and api-only walk one priority list.
            EngineMode::Free | EngineMode::Api => {
                match self
                    .search_priority(
                        query,
                        count,
                        options,
                        self.mode.priority(),
                        &mut accum,
                        observe,
                    )
                    .await?
                {
                    Some(results) => Ok(results),
                    None => accum.exhausted(),
                }
            }
            // Hybrid: free engines first; when the free phase serves nothing
            // (all blocked/empty/failed — including a wait-loop deadline
            // error), fall back to the paid API backends.
            EngineMode::Hybrid => {
                if let Ok(Some(results)) = self
                    .search_priority(
                        query,
                        count,
                        options,
                        &SearchEngine::FREE_PRIORITY,
                        &mut accum,
                        observe,
                    )
                    .await
                {
                    return Ok(results);
                }
                // Yielded nothing (or every engine busy past the wait
                // deadline): the paid phase still gets its chance.
                match self
                    .search_priority(
                        query,
                        count,
                        options,
                        &SearchEngine::API_PRIORITY,
                        &mut accum,
                        observe,
                    )
                    .await
                {
                    Ok(Some(results)) => Ok(results),
                    Ok(None) => accum.exhausted(),
                    Err(e) => Err(e),
                }
            }
        }
    }

    /// Run one fallback phase over `priority`: pick-and-reserve engines in
    /// order, dispatching until one serves non-empty results (`Some`) or the
    /// list is exhausted (`None`).
    ///
    /// Rate-limit/captcha outcomes put the engine into cooldown via
    /// [`Self::record_dispatch_outcome`] — it is never retried immediately.
    /// A quota-exhausted pick is surfaced immediately (waiting cannot
    /// replenish the aggregate quota). All outcomes are recorded into `accum`,
    /// which is shared across the free and paid phases so the final
    /// [`FallbackAccum::exhausted`] verdict reflects the whole search.
    async fn search_priority(
        &self,
        query: &str,
        count: usize,
        options: &SearchOptions,
        priority: &'static [SearchEngine],
        accum: &mut FallbackAccum,
        observe: Option<DispatchObserver<'_>>,
    ) -> Result<Option<Vec<EngineSearchResult>>, SearchEngineError> {
        loop {
            let engine = {
                // Pick **and reserve** the engine atomically: the pick and the
                // in-memory last-call stamp all happen under one lock scope
                // inside pick_and_reserve, so two concurrent searches can
                // never both pick the same engine before either has stamped
                // it. No await inside the scope, so the future stays `Send`
                // for the orchestrator's JoinSet::spawn.
                match pick_and_reserve(&self.state, &self.pacing, priority) {
                    Ok(engine) => engine,
                    // Quota is a hard stop: waiting cannot replenish it, and
                    // it must not be masked by an exhausted phase verdict.
                    Err(e @ SearchEngineError::QuotaExceeded { .. }) => return Err(e),
                    Err(e) if accum.errors.is_empty() && !accum.saw_empty_ok => {
                        // All engines busy: don't hard-fail — wait (bounded by
                        // MAX_AUTO_WAIT) for one to become eligible.
                        match wait_for_available_engine(
                            &self.state,
                            &self.pacing,
                            &self.notify,
                            Instant::now() + MAX_AUTO_WAIT,
                            e,
                            priority,
                        )
                        .await
                        {
                            Ok(engine) => engine,
                            Err(e) => return Err(e),
                        }
                    }
                    Err(_) => return Ok(None),
                }
            };
            let result = self.dispatch(engine, query, count, options).await;
            self.record_dispatch_outcome(engine, &result, observe);

            if let Some(results) = accum.record(result) {
                return Ok(Some(results));
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
    pub(crate) async fn search_pinned(
        &self,
        query: &str,
        count: usize,
        engine: SearchEngine,
        options: &SearchOptions,
        observe: Option<DispatchObserver<'_>>,
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

        // Paid backends are gated by the aggregate API quota (the same check
        // pick_and_reserve performs before reserving a paid engine): an
        // exhausted quota refuses the pinned dispatch with
        // [`SearchEngineError::QuotaExceeded`] (mapped to HTTP 429
        // `quota-exceeded` by consumers).
        if engine.is_paid() {
            let pacing = self.pacing.lock().unwrap_or_else(|e| e.into_inner());
            if pacing.quota_exceeded() {
                return Err(SearchEngineError::QuotaExceeded {
                    engine,
                    detail: format!(
                        "aggregate API quota exhausted (spend {}, limit {})",
                        pacing.quota_spend(),
                        pacing.quota_limit()
                    ),
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
        let result = self.dispatch(engine, query, count, options).await;
        self.record_dispatch_outcome(engine, &result, observe);
        result
    }

    /// Record the outcome of a dispatched query: emit tracing, update the
    /// in-memory cooldown state, and stamp the persisted pacing store.
    ///
    /// Shared by auto and pin modes so the post-dispatch bookkeeping stays in
    /// one place. When the outcome carries a cooldown (rate-limit/captcha),
    /// the cooldown and the last-call stamp are written under a single
    /// [`PacingStore`] lock acquisition.
    pub(crate) fn record_dispatch_outcome(
        &self,
        engine: SearchEngine,
        result: &Result<Vec<EngineSearchResult>, SearchEngineError>,
        observe: Option<DispatchObserver<'_>>,
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
        // A dispatched paid backend consumes one request against the
        // aggregate API quota (counted whether the dispatch succeeded or was
        // blocked — the request was still sent to the provider).
        if engine.is_paid() {
            let mut pacing = self.pacing.lock().unwrap_or_else(|e| e.into_inner());
            pacing.bump_quota();
        }
        // Surface the classified outcome to the per-call observer. When a
        // cooldown is applied (rate-limit/captcha) it is reported alongside
        // the block kind so consumers see the engine cooling down too.
        if let Some(observe) = observe {
            match result {
                Err(SearchEngineError::RateLimited { .. }) => {
                    observe(engine, DispatchOutcome::RateLimited);
                    observe(engine, DispatchOutcome::Cooldown);
                }
                Err(SearchEngineError::Captcha { .. }) => {
                    observe(engine, DispatchOutcome::Captcha);
                    observe(engine, DispatchOutcome::Cooldown);
                }
                Err(_) => observe(engine, DispatchOutcome::Fallback),
                Ok(results) if results.is_empty() => observe(engine, DispatchOutcome::Fallback),
                // Served: results are emitted post-map by the projection.
                Ok(_) => observe(engine, DispatchOutcome::Served),
            }
        }
    }

    /// Record a cooldown and the last-call timestamp for `engine`. The
    /// cooldown is written first, then the pacing tail (last-call stamp +
    /// notify) is delegated to [`Self::record_pacing`] so both paths share a
    /// single tail.
    fn record_cooldown_and_pacing(&self, engine: SearchEngine, until_ms: u64) {
        let mut pacing = self.pacing.lock().unwrap_or_else(|e| e.into_inner());
        pacing.record_cooldown(engine, until_ms);
        drop(pacing);
        self.record_pacing(engine);
    }

    /// Record the last-call timestamp for `engine` after a dispatch.
    pub(crate) fn record_pacing(&self, engine: SearchEngine) {
        let now_ms = unix_now_ms();
        let mut pacing = self.pacing.lock().unwrap_or_else(|e| e.into_inner());
        pacing.record(engine, now_ms);
        drop(pacing);
        // Wake any auto-mode wait loop: pacing state just changed.
        self.notify.notify_waiters();
    }
}
