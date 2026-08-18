use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::Notify;

use crate::engine::router::MAX_AUTO_WAIT;
use crate::engine::router::select::{
    earliest_available_remaining_ms, min_interval, next_engine, pacing_ready, pacing_remaining_ms,
    pick_and_reserve, record_outcome, seed_cooldowns, unix_now_ms, wait_for_available_engine,
};
use crate::engine::{EngineChoice, SearchEngine, SearchEngineError};

use super::{cold_pacing, cold_state};

#[test]
fn auto_picks_priority_first_when_cold() {
    let state = cold_state();
    assert_eq!(
        next_engine(
            &state,
            &cold_pacing(),
            &EngineChoice::Auto,
            &SearchEngine::FREE_PRIORITY
        )
        .unwrap(),
        SearchEngine::Google
    );
}

#[test]
fn falls_back_after_rate_limited() {
    let mut state = cold_state();
    let error = Err(SearchEngineError::RateLimited {
        engine: SearchEngine::Google,
        detail: "HTTP 429".to_string(),
        retry_after_ms: None,
    });
    record_outcome(&mut state, SearchEngine::Google, &error);
    assert_eq!(
        next_engine(
            &state,
            &cold_pacing(),
            &EngineChoice::Auto,
            &SearchEngine::FREE_PRIORITY
        )
        .unwrap(),
        SearchEngine::Brave
    );
}

#[test]
fn skips_engine_in_captcha_cooldown() {
    let mut state = cold_state();
    state.cooldowns.insert(
        SearchEngine::Google,
        Instant::now() + Duration::from_secs(30 * 60),
    );
    assert_eq!(
        next_engine(
            &state,
            &cold_pacing(),
            &EngineChoice::Auto,
            &SearchEngine::FREE_PRIORITY
        )
        .unwrap(),
        SearchEngine::Brave
    );
}

#[test]
fn skips_engine_over_budget() {
    // Over-budget is enforced via the persisted pacing store (the single
    // source of truth for query budgets): a just-recorded Google call makes
    // auto mode skip it and fall back to Brave.
    let state = cold_state();
    let now_ms = unix_now_ms();
    let mut pacing = cold_pacing();
    pacing.record(SearchEngine::Google, now_ms);
    assert_eq!(
        next_engine(
            &state,
            &pacing,
            &EngineChoice::Auto,
            &SearchEngine::FREE_PRIORITY
        )
        .unwrap(),
        SearchEngine::Brave
    );
}

#[test]
fn all_engines_cooled_yields_unavailable() {
    let mut state = cold_state();
    let future = Instant::now() + Duration::from_secs(3600);
    for engine in SearchEngine::FREE_PRIORITY {
        state.cooldowns.insert(engine, future);
    }
    let err = next_engine(
        &state,
        &cold_pacing(),
        &EngineChoice::Auto,
        &SearchEngine::FREE_PRIORITY,
    )
    .unwrap_err();
    match err {
        SearchEngineError::Unavailable { engine, detail } => {
            assert_eq!(engine, SearchEngine::Google);
            assert!(detail.contains("all engines cooling down or over budget"));
        }
        other => panic!("expected Unavailable, got {other:?}"),
    }
}

#[test]
fn pin_forces_pinned_engine_even_when_not_first() {
    let state = cold_state();
    assert_eq!(
        next_engine(
            &state,
            &cold_pacing(),
            &EngineChoice::Pin(SearchEngine::Google),
            &SearchEngine::FREE_PRIORITY
        )
        .unwrap(),
        SearchEngine::Google
    );
    assert_eq!(
        next_engine(
            &state,
            &cold_pacing(),
            &EngineChoice::Pin(SearchEngine::Bing),
            &SearchEngine::FREE_PRIORITY
        )
        .unwrap(),
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
    assert_eq!(
        pacing_remaining_ms(Some(1_000_000), 4000, 1_003_000),
        Some(1000)
    );
    // Zero elapsed → wait the full interval.
    assert_eq!(
        pacing_remaining_ms(Some(1_000_000), 4000, 1_000_000),
        Some(4000)
    );
    // A timestamp in the future (clock skew) → wait the full interval.
    assert_eq!(
        pacing_remaining_ms(Some(2_000_000), 4000, 1_000_000),
        Some(4000)
    );
}

#[test]
fn auto_skips_engine_with_recent_persisted_call() {
    // Google was called 1s ago (6s interval not elapsed) — the persisted
    // pacing state must make auto mode skip it just like an over-budget
    // engine, falling back to Brave.
    let state = cold_state();
    let now_ms = unix_now_ms();
    let mut pacing = cold_pacing();
    pacing.record(SearchEngine::Google, now_ms - 1000);
    assert_eq!(
        next_engine(
            &state,
            &pacing,
            &EngineChoice::Auto,
            &SearchEngine::FREE_PRIORITY
        )
        .unwrap(),
        SearchEngine::Brave
    );
}

#[test]
fn auto_skips_engine_until_persisted_interval_elapses() {
    let state = cold_state();
    let now_ms = unix_now_ms();
    let mut pacing = cold_pacing();
    pacing.record(SearchEngine::Google, now_ms - 1000);
    // Still within the 6s Google interval: skipped, falls to Brave.
    assert_eq!(
        next_engine(
            &state,
            &pacing,
            &EngineChoice::Auto,
            &SearchEngine::FREE_PRIORITY
        )
        .unwrap(),
        SearchEngine::Brave
    );
    // Once 6s have elapsed the engine is eligible again and wins.
    pacing.record(SearchEngine::Google, now_ms - 61_000);
    assert_eq!(
        next_engine(
            &state,
            &pacing,
            &EngineChoice::Auto,
            &SearchEngine::FREE_PRIORITY
        )
        .unwrap(),
        SearchEngine::Google
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
        earliest_available_remaining_ms(
            &cold_state(),
            &pacing,
            Instant::now(),
            now_ms,
            &SearchEngine::FREE_PRIORITY
        ),
        Some(1000),
        "min remaining across engines wins (Bing's 1s interval)"
    );
}

#[test]
fn earliest_remaining_none_when_all_engines_eligible() {
    let now_ms = 1_000_000;
    assert_eq!(
        earliest_available_remaining_ms(
            &cold_state(),
            &cold_pacing(),
            Instant::now(),
            now_ms,
            &SearchEngine::FREE_PRIORITY
        ),
        None,
        "cold pacing: every engine may be dispatched right away"
    );
    // Interval fully elapsed on every engine ⇒ still None.
    let mut pacing = cold_pacing();
    pacing.record(SearchEngine::Brave, now_ms - 60_000);
    pacing.record(SearchEngine::Bing, now_ms - 1_000);
    pacing.record(SearchEngine::Google, now_ms - 6_000);
    assert_eq!(
        earliest_available_remaining_ms(
            &cold_state(),
            &pacing,
            Instant::now(),
            now_ms,
            &SearchEngine::FREE_PRIORITY
        ),
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
        earliest_available_remaining_ms(
            &cold_state(),
            &pacing,
            Instant::now(),
            now_ms,
            &SearchEngine::FREE_PRIORITY
        ),
        Some(10_000)
    );
    // An expired cooldown no longer blocks.
    pacing.record_cooldown(SearchEngine::Google, now_ms - 1_000);
    assert_eq!(
        earliest_available_remaining_ms(
            &cold_state(),
            &pacing,
            Instant::now(),
            now_ms,
            &SearchEngine::FREE_PRIORITY
        ),
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
        earliest_available_remaining_ms(
            &state,
            &cold_pacing(),
            now,
            now_ms,
            &SearchEngine::FREE_PRIORITY
        ),
        Some(30_000),
        "in-memory cooldown dominates the earliest remaining"
    );
    // An expired in-memory cooldown no longer blocks.
    state
        .cooldowns
        .insert(SearchEngine::Brave, now - Duration::from_secs(1));
    assert_eq!(
        earliest_available_remaining_ms(
            &state,
            &cold_pacing(),
            now,
            now_ms,
            &SearchEngine::FREE_PRIORITY
        ),
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
        for engine in SearchEngine::FREE_PRIORITY {
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
        &SearchEngine::FREE_PRIORITY,
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
    // Brave and Google are cooled down for an hour; Bing only
    // needs its 1s minimum interval to elapse. The loop must wait it out
    // (~1s, sleeping the full remaining interval) and then dispatch to Bing.
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
        .record(SearchEngine::Bing, unix_now_ms());
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
        &SearchEngine::FREE_PRIORITY,
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
    let engine = pick_and_reserve(&state, &pacing, &SearchEngine::FREE_PRIORITY)
        .expect("cold state: Google eligible");
    assert_eq!(engine, SearchEngine::Google);
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
    // to the second pick (Google's 6s interval is nowhere near elapsed),
    // so the two picks must land on different engines — a pre-fix race
    // would hand both tasks Google and double-dispatch the server.
    let state = Arc::new(Mutex::new(cold_state()));
    let pacing = Arc::new(Mutex::new(cold_pacing()));
    let (s1, p1) = (state.clone(), pacing.clone());
    let (s2, p2) = (state.clone(), pacing.clone());
    let (a, b) = tokio::join!(
        tokio::task::spawn_blocking(move || {
            pick_and_reserve(&s1, &p1, &SearchEngine::FREE_PRIORITY)
        }),
        tokio::task::spawn_blocking(move || {
            pick_and_reserve(&s2, &p2, &SearchEngine::FREE_PRIORITY)
        }),
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
    // Brave and Google are cooled down for an hour; Bing's 1s
    // interval has already elapsed, so the first poll picks it. The pick
    // must stamp the store (reserve) at pick time, not merely report it.
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
        &SearchEngine::FREE_PRIORITY,
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
    // Bing's 1s interval just started; Brave and Google are
    // cooled down for an hour. The wait loop must sleep the full ~1s
    // remaining (not a 500ms poll step) before Bing becomes eligible.
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
        .record(SearchEngine::Bing, unix_now_ms());
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
        &SearchEngine::FREE_PRIORITY,
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
fn seeded_cooldown_makes_auto_skip_engine() {
    // A cooldown persisted by a previous invocation (e.g. a rate-limit)
    // is seeded into the in-memory map on router construction; auto mode
    // must skip the engine until it expires.
    let now = Instant::now();
    let now_ms = unix_now_ms();
    let mut pacing = cold_pacing();
    pacing.record_cooldown(SearchEngine::Google, now_ms + 60_000);
    let mut state = cold_state();
    state.cooldowns = seed_cooldowns(&pacing, now, now_ms);
    assert_eq!(
        next_engine(
            &state,
            &pacing,
            &EngineChoice::Auto,
            &SearchEngine::FREE_PRIORITY
        )
        .unwrap(),
        SearchEngine::Brave
    );
}

#[test]
fn seeded_cooldown_expired_is_dropped() {
    // An expired persisted cooldown must not block the engine.
    let now = Instant::now();
    let now_ms = unix_now_ms();
    let mut pacing = cold_pacing();
    pacing.record_cooldown(SearchEngine::Google, now_ms - 1000);
    let mut state = cold_state();
    state.cooldowns = seed_cooldowns(&pacing, now, now_ms);
    assert!(state.cooldowns.is_empty());
    assert_eq!(
        next_engine(
            &state,
            &pacing,
            &EngineChoice::Auto,
            &SearchEngine::FREE_PRIORITY
        )
        .unwrap(),
        SearchEngine::Google
    );
}

#[test]
fn rate_limit_outcome_reports_cooldown_to_persist() {
    let mut state = cold_state();
    let error = Err(SearchEngineError::RateLimited {
        engine: SearchEngine::Google,
        detail: "HTTP 429".to_string(),
        retry_after_ms: None,
    });
    let (engine, until_ms) = record_outcome(&mut state, SearchEngine::Google, &error)
        .expect("rate limit must report a persisted cooldown");
    assert_eq!(engine, SearchEngine::Google);
    assert!(
        until_ms > unix_now_ms(),
        "cooldown until-ms must be in the future"
    );
    // In-memory cooldown still set (existing behavior unchanged).
    assert_eq!(
        next_engine(
            &state,
            &cold_pacing(),
            &EngineChoice::Auto,
            &SearchEngine::FREE_PRIORITY
        )
        .unwrap(),
        SearchEngine::Brave
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
    pacing
        .lock()
        .unwrap()
        .record(SearchEngine::Bing, unix_now_ms());
    let notify = Arc::new(Notify::new());

    // A sibling task "dispatches" Bing after 100ms: stamps a fresh
    // last-call (making it eligible) and wakes the waiter.
    let (p2, n2) = (pacing.clone(), notify.clone());
    let waker = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        p2.lock()
            .unwrap()
            .record(SearchEngine::Bing, unix_now_ms() - 2000);
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
        &SearchEngine::FREE_PRIORITY,
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
fn hybrid_priority_falls_back_to_paid_when_free_blocked() {
    // Hybrid mode: every free engine blocked into cooldown (a rate-limit
    // or captcha block) must route the next auto pick to the first paid
    // API backend instead of failing.
    let mut state = cold_state();
    let future = Instant::now() + Duration::from_secs(3600);
    for engine in SearchEngine::FREE_PRIORITY {
        state.cooldowns.insert(engine, future);
    }
    assert_eq!(
        next_engine(
            &state,
            &cold_pacing(),
            &EngineChoice::Auto,
            &SearchEngine::HYBRID_PRIORITY
        )
        .unwrap(),
        SearchEngine::BraveApi,
        "free phase fully blocked: fall back to the paid backends"
    );
}

#[test]
fn hybrid_priority_429_outcomes_fall_back_to_paid() {
    // Real rate-limit (429) outcomes recorded via record_outcome must put
    // every free engine into cooldown, so the hybrid pick lands on the
    // first paid backend — the exact search_auto free→paid phase path.
    let mut state = cold_state();
    for engine in SearchEngine::FREE_PRIORITY {
        let error = Err(SearchEngineError::RateLimited {
            engine,
            detail: "HTTP 429".to_string(),
            retry_after_ms: None,
        });
        record_outcome(&mut state, engine, &error);
    }
    assert_eq!(
        next_engine(
            &state,
            &cold_pacing(),
            &EngineChoice::Auto,
            &SearchEngine::HYBRID_PRIORITY
        )
        .unwrap(),
        SearchEngine::BraveApi
    );
}

#[test]
fn hybrid_priority_skips_blocked_paid_backend_too() {
    // BraveApi cooled down as well: the hybrid fallback must walk past it
    // to the next paid backend (Tavily), never returning a blocked engine.
    let mut state = cold_state();
    let future = Instant::now() + Duration::from_secs(3600);
    for engine in SearchEngine::FREE_PRIORITY {
        state.cooldowns.insert(engine, future);
    }
    state.cooldowns.insert(SearchEngine::BraveApi, future);
    assert_eq!(
        next_engine(
            &state,
            &cold_pacing(),
            &EngineChoice::Auto,
            &SearchEngine::HYBRID_PRIORITY
        )
        .unwrap(),
        SearchEngine::Tavily
    );
}

#[test]
fn quota_exceeded_blocks_paid_pick_and_reserve() {
    // Exhaust the aggregate paid-API quota; a paid pick must refuse with
    // QuotaExceeded instead of reserving (spend is only counted once a
    // dispatch is actually reserved), while the free phase is unaffected.
    let state = Mutex::new(cold_state());
    let pacing = Mutex::new(cold_pacing());
    {
        let mut pacing = pacing.lock().unwrap();
        while !pacing.quota_exceeded() {
            pacing.bump_quota();
        }
    }
    match pick_and_reserve(&state, &pacing, &SearchEngine::API_PRIORITY) {
        Err(SearchEngineError::QuotaExceeded { engine, detail }) => {
            assert_eq!(engine, SearchEngine::BraveApi);
            assert!(detail.contains("quota"));
        }
        other => panic!("expected QuotaExceeded, got {other:?}"),
    }
    // Free engines are never gated by the paid-API quota.
    assert_eq!(
        pick_and_reserve(&state, &pacing, &SearchEngine::FREE_PRIORITY).unwrap(),
        SearchEngine::Google
    );
}
