//! Streaming search core.
//!
//! [`search_streaming`] runs a search through a shared [`SearchRouter`] and
//! emits progressive [`SearchEvent`]s over an mpsc channel, so the serve daemon
//! can project them to SSE. The router's dispatch-outcome funnel
//! ([`SearchRouter::record_dispatch_outcome`]) is observed per-call (threaded,
//! never stored on the router) so concurrent searches on a shared router —
//! e.g. the batch path — never clobber each other's observer.
//!
//! This module is the single search path: [`crate::search::search_with_router`]
//! is a projection that consumes the same event stream and collects the
//! [`SearchEvent::Result`] events into a plain `Vec`.

use std::sync::Arc;
use std::time::Instant;

use tokio::sync::mpsc;

use crate::engine::router::{DispatchOutcome, SearchRouter, map_engine_results};
use crate::engine::{EngineChoice, SearchEngine, SearchOptions};
use crate::stream::{EngineEventKind, SearchEvent, Sender};

/// Produce the [`SearchEvent`] stream for one query into `tx`.
///
/// Honest emission contract:
/// - [`SearchEvent::JobStarted`] is emitted immediately.
/// - [`SearchEvent::Result`] events are emitted **post-map** (after
///   [`map_engine_results`], which applies filtering/dedup/renumber), so
///   position and dedup semantics match the collect facade exactly.
/// - [`SearchEvent::EngineEvent`] events are emitted from the router's
///   dispatch-outcome funnel as engines are tried (fallback / rate-limit /
///   captcha / cooldown).
/// - Exactly one terminal event is emitted: [`SearchEvent::Done`] on success,
///   [`SearchEvent::Error`] on failure.
///
/// The producer aborts early (stops sending) when the consumer drops `tx` —
/// the receiver end of the channel being closed is backpressure in reverse.
pub(crate) async fn search_streaming_into(
    tx: Sender,
    router: Arc<SearchRouter>,
    query: &str,
    count: usize,
    choice: EngineChoice,
    options: &SearchOptions,
) {
    let start = Instant::now();
    let _ = tx.send(SearchEvent::JobStarted).await;

    // Per-call observer: map each classified dispatch outcome to an
    // EngineEvent. `try_send` because the observer is synchronous (it runs
    // under the router's outcome funnel, off the async executor); engine
    // events are tiny and few (≤ 10), so the bounded channel swallows them.
    let observer = |engine: SearchEngine, outcome: DispatchOutcome| {
        let kind = match outcome {
            // Served engines need no event: their results are emitted
            // post-map as `Result` events.
            DispatchOutcome::Served => return,
            DispatchOutcome::Fallback => EngineEventKind::Fallback,
            DispatchOutcome::RateLimited => EngineEventKind::RateLimited,
            DispatchOutcome::Captcha => EngineEventKind::Captcha,
            DispatchOutcome::Cooldown => EngineEventKind::Cooldown,
        };
        let _ = tx.try_send(SearchEvent::EngineEvent { engine, kind });
    };

    let outcome = router
        .search_with_fallback_observed(query, count, choice, options, Some(&observer))
        .await;

    match outcome {
        Ok(results) => {
            let duration_ms = start.elapsed().as_millis() as u64;
            for result in map_engine_results(results, query, duration_ms, router.mode()) {
                if tx
                    .send(SearchEvent::Result(Box::new(result)))
                    .await
                    .is_err()
                {
                    return; // consumer gone; nothing more to emit
                }
            }
            let _ = tx.send(SearchEvent::Done).await;
        }
        Err(e) => {
            let _ = tx.send(SearchEvent::Error(e)).await;
        }
    }
}

/// Run a search in the background and return its [`SearchEvent`] stream.
///
/// The caller owns the returned [`mpsc::Receiver`]; the producer task runs
/// concurrently and aborts when the receiver is dropped. `router` is taken by
/// [`Arc`] so the spawned task is `'static`; callers already hold the shared
/// router as an `Arc` (see [`crate::batch::BatchProcessor`]).
pub fn search_streaming(
    router: Arc<SearchRouter>,
    query: String,
    count: usize,
    choice: EngineChoice,
    options: &SearchOptions,
) -> mpsc::Receiver<SearchEvent> {
    let (tx, rx) = mpsc::channel(64);
    let options = options.clone();
    tokio::spawn(async move {
        search_streaming_into(tx, router, &query, count, choice, &options).await;
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SearchResult;
    use crate::engine::pacing::PacingStore;
    use crate::engine::{
        EngineSearchResult, SearchEngineBackend, SearchEngineError, SearchOptions,
    };

    /// Test-only backend returning a canned outcome for any dispatch, injected
    /// through the router's `cfg(test)` fake-backend seam.
    struct FakeBackend {
        engine: SearchEngine,
        mode: FakeMode,
    }

    enum FakeMode {
        Results(Vec<EngineSearchResult>),
        RateLimited,
    }

    impl SearchEngineBackend for FakeBackend {
        fn name(&self) -> SearchEngine {
            self.engine
        }

        async fn search(
            &self,
            _query: &str,
            _count: usize,
            _options: &SearchOptions,
        ) -> Result<Vec<EngineSearchResult>, SearchEngineError> {
            match &self.mode {
                FakeMode::Results(results) => Ok(results.clone()),
                FakeMode::RateLimited => Err(SearchEngineError::RateLimited {
                    engine: self.engine,
                    detail: "test 429".to_string(),
                    retry_after_ms: None,
                }),
            }
        }
    }

    fn fake_router(mode: FakeMode) -> Arc<SearchRouter> {
        let fake = Arc::new(FakeBackend {
            engine: SearchEngine::Brave,
            mode,
        });
        // Box the canned search into the dyn-compatible seam (the real
        // `SearchEngineBackend` trait has an `async fn` and cannot be `dyn`).
        let boxed: crate::engine::router::BoxedSearch = Arc::new(move |query, count, options| {
            let fake = Arc::clone(&fake);
            Box::pin(async move { fake.search(query, count, options).await })
        });
        Arc::new(SearchRouter::with_fake_backend(
            Arc::new(std::sync::Mutex::new(PacingStore::new())),
            boxed,
        ))
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

    async fn collect(router: Arc<SearchRouter>, choice: EngineChoice) -> Vec<SearchEvent> {
        let (tx, mut rx) = mpsc::channel(16);
        search_streaming_into(
            tx,
            router,
            "rust streams",
            5,
            choice,
            &SearchOptions::default(),
        )
        .await;
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        events
    }

    #[tokio::test]
    async fn streaming_emits_job_started_results_done() {
        let router = fake_router(FakeMode::Results(vec![
            engine_result("Hit A", "https://a.example", "snippet a"),
            engine_result("Hit B", "https://b.example", "snippet b"),
        ]));
        let events = collect(router, EngineChoice::Auto).await;

        assert!(
            matches!(events.first(), Some(SearchEvent::JobStarted)),
            "stream must open with JobStarted"
        );
        let results: Vec<&SearchResult> = events
            .iter()
            .filter_map(|e| match e {
                SearchEvent::Result(r) => Some(r.as_ref()),
                _ => None,
            })
            .collect();
        assert_eq!(results.len(), 2, "one Result event per mapped result");
        assert_eq!(results[0].title, "Hit A");
        assert_eq!(results[0].position, 1, "positions renumbered post-map");
        assert_eq!(results[1].title, "Hit B");
        assert_eq!(results[1].position, 2);
        assert!(
            matches!(events.last(), Some(SearchEvent::Done)),
            "stream must terminate with Done"
        );
    }

    #[tokio::test]
    async fn streaming_emits_engine_event_on_rate_limit() {
        let router = fake_router(FakeMode::RateLimited);
        let events = collect(router, EngineChoice::Pin(SearchEngine::Brave)).await;

        assert!(
            events.iter().any(|e| matches!(
                e,
                SearchEvent::EngineEvent {
                    engine: SearchEngine::Brave,
                    kind: EngineEventKind::RateLimited,
                    ..
                }
            )),
            "a rate-limited engine must surface an EngineEvent(rate_limited)"
        );
        assert!(
            events.iter().any(|e| matches!(
                e,
                SearchEvent::EngineEvent {
                    engine: SearchEngine::Brave,
                    kind: EngineEventKind::Cooldown,
                    ..
                }
            )),
            "entering a cooldown must surface an EngineEvent(cooldown)"
        );
        assert!(
            matches!(
                events.last(),
                Some(SearchEvent::Error(SearchEngineError::RateLimited { .. }))
            ),
            "a failed stream must terminate with Error"
        );
    }

    #[tokio::test]
    async fn streaming_emits_job_started_done_for_empty_results() {
        let router = fake_router(FakeMode::Results(vec![]));
        let events = collect(router, EngineChoice::Pin(SearchEngine::Brave)).await;

        assert!(matches!(events.first(), Some(SearchEvent::JobStarted)));
        assert!(
            matches!(events.last(), Some(SearchEvent::Done)),
            "an empty-but-healthy search must terminate with Done"
        );
    }
}
