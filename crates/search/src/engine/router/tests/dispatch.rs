use std::sync::{Arc, Mutex};

use crate::engine::router::SearchRouter;
use crate::engine::router::select::FallbackAccum;
use crate::engine::{EngineSearchResult, SearchEngine, SearchEngineError, SearchOptions};

use super::{cold_pacing, engine_result};

#[test]
fn record_dispatch_outcome_stamps_pacing_on_success() {
    // Auto and pin modes share record_dispatch_outcome; a success must
    // stamp the persisted last-call so the next dispatch is paced.
    let router = SearchRouter::with_pacing(None, Arc::new(Mutex::new(cold_pacing())));
    let result: Result<Vec<EngineSearchResult>, SearchEngineError> = Ok(vec![]);
    router.record_dispatch_outcome(SearchEngine::Bing, &result, None);
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
            retry_after_ms: None,
        });
    router.record_dispatch_outcome(SearchEngine::Brave, &result, None);
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
    for _ in SearchEngine::FREE_PRIORITY {
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
    assert!(
        accum
            .record(Err(SearchEngineError::RateLimited {
                engine: SearchEngine::Brave,
                detail: "HTTP 429".to_string(),
                retry_after_ms: None,
            }))
            .is_none()
    );
    match accum.exhausted().unwrap_err() {
        SearchEngineError::AllEnginesFailed(errors) => assert_eq!(errors.len(), 1),
        other => panic!("expected AllEnginesFailed, got {other:?}"),
    }
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
        r2.pacing
            .lock()
            .unwrap()
            .last_call_ms(SearchEngine::Bing)
            .is_some(),
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
        r2.pacing
            .lock()
            .unwrap()
            .last_call_ms(SearchEngine::Bing)
            .is_some(),
        "routers built via new() must share the global pacing store"
    );
}

#[tokio::test]
async fn pinned_paid_engine_quota_exceeded() {
    // search_pinned gates paid backends behind the aggregate quota: an
    // exhausted quota refuses the pinned dispatch before any pacing wait
    // or HTTP call.
    let pacing = Arc::new(Mutex::new(cold_pacing()));
    {
        let mut pacing = pacing.lock().unwrap();
        while !pacing.quota_exceeded() {
            pacing.bump_quota();
        }
    }
    let router = SearchRouter::with_pacing(None, pacing);
    let err = router
        .search_pinned(
            "rust",
            5,
            SearchEngine::Tavily,
            &SearchOptions::default(),
            None,
        )
        .await
        .unwrap_err();
    match err {
        SearchEngineError::QuotaExceeded { engine, .. } => {
            assert_eq!(engine, SearchEngine::Tavily);
        }
        other => panic!("expected QuotaExceeded, got {other:?}"),
    }
}
