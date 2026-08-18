//! REQUESTS-PER-MINUTE benchmark v2: TRUE server-side rate-limit ceilings with
//! PACING UNLOCKED.
//!
//! v1 measured sustained RPM at artificial pacing (6s/5s/1s per engine) — and
//! the pacing itself PREVENTED ever reaching the rate limit (google 4.3 rpm,
//! brave 5.4 rpm, bing 49.5 rpm, all 20/20 clean). This rewrite hammers each
//! engine with BOTH stress strategies at minimal delay to discover where the
//! engine itself starts blocking:
//!
//! 1. **PARALLEL phase** (per engine, first): fan out `PARALLEL_PER_BATCH`
//!    (default 4) concurrent queries per batch through the RAW backends
//!    (`GoogleBackend` / `BraveBackend` / `BingBackend::search`) — NOT through
//!    the SearchRouter, so no in-process pacing gates (Google ~6s / Brave ~5s /
//!    Bing ~1s) throttle anything. Concurrency mirrors `crate::batch`'s
//!    JoinSet + shared-semaphore shape (batch.rs:82-115). Run `N` batches
//!    (`GTHINGS_BENCH_N`, default 8) with a minimal delay between batches
//!    (default 500 ms). STOP at the first batch where ALL queries block
//!    (empty / captcha / rate_limited / error) — that is the rate-limit
//!    signal. Recovery probe after.
//! 2. **HARVEST phase** (per engine, second): loop `harvest()` with the engine
//!    pinned (`BatchHarvestRequest { queries: [distinct query], engine:
//!    Some(SearchEngine::X), follow_top_n, max_chars }`), minimal delay
//!    between iterations (~500 ms default). Harvest swallows engine search
//!    errors to empty (orchestrator/search.rs:95-98), so 0 harvested results
//!    = block signal. STOP at the first block. Recovery probe after.
//!
//! Engine order: Google → Brave → Bing (each engine fully
//! isolated before the next starts). Per engine per strategy the report gives total requests,
//! successes, first-block point, block type, recovery verdict (transient/hard)
//! and the implied requests-per-minute at the point of blocking.
//!
//! NOTE: the parallel phase is the truly pacing-unlocked measurement. The
//! harvest phase dispatches through the router's pinned path, which politely
//! waits out the engine's minimum interval per call — its implied RPM shows
//! what the router-mediated path tolerates.
//!
//! Requires a LIVE Chrome on 127.0.0.1:9222 (`--remote-debugging-port=9222`).
//! Ignored by default so normal CI does not run it; execute explicitly with:
//!
//! ```sh
//! cargo test -p gthings-search --test rpm -- --ignored --nocapture
//! ```
//!
//! Configurable via env vars (defaults shown):
//! - `GTHINGS_BENCH_N`            — batches per parallel phase AND iterations
//!   per harvest phase (default 8; 4 queries/batch ⇒ 32 requests per parallel
//!   phase)
//! - `GTHINGS_BENCH_PARALLEL`     — concurrent queries per batch (default 4)
//! - `GTHINGS_BENCH_FOLLOW_TOP`   — follow_top_n per harvest iteration
//!   (default 1)
//! - `GTHINGS_BENCH_DELAY_MS`     — delay between batches/iterations
//!   (default 500; the point is to UNLOCK pacing and find the true limit)
//! - `GTHINGS_BENCH_CDP_PORT`     — CDP port (default 9222)
//!
//! No assertions — the benchmark reports what actually happened. With pacing
//! unlocked the test can take several minutes; lower `GTHINGS_BENCH_N` to
//! shorten it.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gthings_cdp::{Session, detect};
use gthings_common::pagination::ExtractParams;
use gthings_search::engine::{
    BingBackend, BraveBackend, EngineSearchResult, GoogleBackend, SearchEngine, SearchOptions,
};
use gthings_search::harvest::{BatchHarvestRequest, RankStrategy, harvest};
use gthings_search::{SearchEngineBackend, SearchEngineError};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

/// Distinct queries, one per request, to avoid engines serving a cached SERP
/// for a repeated query.
const QUERIES: &[&str] = &[
    "rust programming",
    "tokio async runtime",
    "axum web framework",
    "web scraping best practices",
    "machine learning tutorial",
    "postgresql indexing",
    "kubernetes networking",
    "typescript generics",
    "react server components",
    "sqlite performance tuning",
    "distributed systems design",
    "zero copy serialization",
    "observability best practices",
    "memory safe languages",
    "http caching strategies",
    "database sharding patterns",
    "concurrency in go",
    "event driven architecture",
    "api rate limiting design",
    "vector databases comparison",
    "cargo workspace monorepo",
    "websocket protocol explained",
    "semantic versioning guide",
    "test driven development",
    "functional programming rust",
    "llm prompt engineering",
    "cdn edge computing",
    "oauth2 authorization code flow",
    "container security hardening",
    "quantum computing basics",
];

/// Concurrent queries per parallel batch — mirrors `batch.rs`'s
/// MAX_CONCURRENT_TABS.
const DEFAULT_PARALLEL_PER_BATCH: usize = 4;
/// `follow_top_n` used by each harvest iteration.
const DEFAULT_HARVEST_FOLLOW_TOP_N: usize = 1;
/// `max_chars` used by each harvest iteration's follow.
const HARVEST_MAX_CHARS: usize = 3000;

/// Stress strategy under test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Strategy {
    Parallel,
    Harvest,
}

impl Strategy {
    fn as_str(&self) -> &'static str {
        match self {
            Strategy::Parallel => "parallel",
            Strategy::Harvest => "harvest",
        }
    }
}

/// Per-engine per-strategy report.
#[derive(Debug)]
struct PhaseReport {
    engine: SearchEngine,
    strategy: Strategy,
    /// Total engine search requests sent.
    total_requests: usize,
    successes: usize,
    /// 1-based batch/iteration index of the first block, if any.
    first_block_at: Option<usize>,
    block_type: String,
    recovery: String,
    /// Requests per minute actually sent at the point of stopping (block or
    /// budget exhaustion).
    implied_rpm: f64,
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn env_u16(name: &str) -> Option<u16> {
    std::env::var(name).ok().and_then(|v| v.parse().ok())
}

/// Classify a backend error into a short block type string.
fn error_type(err: &SearchEngineError) -> &'static str {
    match err {
        SearchEngineError::Captcha { .. } => "captcha",
        SearchEngineError::RateLimited { .. } => "rate_limited",
        SearchEngineError::Network { .. } => "network",
        SearchEngineError::Parse { .. } => "parse",
        SearchEngineError::Unavailable { .. } => "unavailable",
        SearchEngineError::QuotaExceeded { .. } => "quota",
        SearchEngineError::AllEnginesFailed(_) => "all_engines_failed",
    }
}

/// Is `outcome` a block (as opposed to a success)?
fn is_block(outcome: &str) -> bool {
    !outcome.starts_with("success(")
}

/// Short "3x empty, 1x rate_limited" summary of the block types in a batch.
fn summarize_blocks(outcomes: &[(String, String, u64)]) -> String {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for (_, outcome, _) in outcomes {
        if !is_block(outcome) {
            continue;
        }
        let kind = outcome
            .trim_start_matches("block(")
            .trim_end_matches(')')
            .to_string();
        if let Some((_, c)) = counts.iter_mut().find(|(k, _)| *k == kind) {
            *c += 1;
        } else {
            counts.push((kind, 1));
        }
    }
    counts
        .into_iter()
        .map(|(k, c)| format!("{c}x{k}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Connect to the live Chrome via CDP detection + Session::connect.
async fn connect(port: u16) -> Session {
    let detected = detect(port).await.unwrap_or_else(|e| {
        panic!("No Chrome on port {port} (need --remote-debugging-port={port}): {e}")
    });
    eprintln!(
        "[bench] detected {}/{} at {}",
        detected.browser, detected.version, detected.ws_url
    );
    Session::connect(&detected.ws_url, Some(Duration::from_secs(30)))
        .await
        .unwrap_or_else(|e| panic!("Session::connect failed on {}: {e}", detected.ws_url))
}

/// Classify a raw backend result into an outcome string.
fn classify(result: Result<Vec<EngineSearchResult>, SearchEngineError>) -> String {
    match result {
        Ok(results) if results.is_empty() => "block(empty)".to_string(),
        Ok(results) => format!("success({} results)", results.len()),
        Err(e) => format!("block({})", error_type(&e)),
    }
}

/// One immediate recovery probe after a block: a fresh raw backend search to
/// classify the block as transient (engine recovered) vs hard (persistent).
async fn run_recovery_probe(
    engine: SearchEngine,
    session: &Arc<Session>,
    first_block_at: Option<usize>,
) -> String {
    let mut recovery = "not run (no block)".to_string();
    if let Some(block_at) = first_block_at {
        let probe_query = "postgresql vs mysql comparison";
        println!();
        println!("--- recovery probe: \"{probe_query}\" (after block at {block_at}) ---");
        let probe_start = Instant::now();
        let options = SearchOptions::default();
        let probe = match engine {
            SearchEngine::Google => classify(
                GoogleBackend::new(Arc::clone(session))
                    .search(probe_query, 10, &options)
                    .await,
            ),
            SearchEngine::Brave => classify(
                BraveBackend::new(Arc::clone(session))
                    .search(probe_query, 10, &options)
                    .await,
            ),
            SearchEngine::Bing => {
                classify(BingBackend::new().search(probe_query, 10, &options).await)
            }
            other => format!("block(unsupported_engine:{other:?})"),
        };
        recovery = format!(
            "{} ({} ms) — {}",
            probe,
            probe_start.elapsed().as_millis(),
            if is_block(&probe) {
                "HARD: engine still blocked"
            } else {
                "TRANSIENT: engine recovered"
            }
        );
        println!("    => {recovery}");
    }
    recovery
}

/// Build a [`PhaseReport`] from raw counters and print its summary block.
#[allow(clippy::too_many_arguments)]
fn finalize_report(
    engine: SearchEngine,
    strategy: Strategy,
    units_run: usize,
    total_requests: usize,
    successes: usize,
    first_block_at: Option<usize>,
    block_type: String,
    recovery: String,
    started: Instant,
    budget: usize,
) -> PhaseReport {
    let elapsed_secs = started.elapsed().as_secs_f64();
    let implied_rpm = if elapsed_secs > 0.0 {
        total_requests as f64 / elapsed_secs * 60.0
    } else {
        0.0
    };

    println!();
    println!("== SUMMARY ({engine:?} / {}) ==", strategy.as_str());
    println!("units_run           : {units_run} (budget {budget})");
    println!("total_requests      : {total_requests}");
    println!("successes           : {successes}");
    println!("failures/blocked    : {}", total_requests - successes);
    match &first_block_at {
        Some(n) => println!("first_block_at      : {n} (type: {block_type})"),
        None => println!("first_block_at      : none (all requests succeeded)"),
    }
    println!("elapsed             : {elapsed_secs:.1} s");
    match &first_block_at {
        Some(n) => println!(
            "IMPLIED RPM AT BLOCK: {implied_rpm:.1} req/min (blocked at {total_requests} requests in {elapsed_secs:.0} s, batch/iter #{n})"
        ),
        None => println!(
            "NO BLOCK in {total_requests} requests (≈ {implied_rpm:.1} req/min sustained over {elapsed_secs:.0} s)"
        ),
    }
    println!("recovery_probe      : {recovery}");
    println!();

    PhaseReport {
        engine,
        strategy,
        total_requests,
        successes,
        first_block_at,
        block_type,
        recovery,
        implied_rpm,
    }
}

/// PARALLEL phase: `max_batches` batches of `per_batch` concurrent queries
/// straight into the raw backend (no router → no in-process pacing gates),
/// `delay_ms` between batches. Stops at the first batch where ALL queries
/// block, then runs a recovery probe.
async fn run_parallel_phase(
    engine: SearchEngine,
    session: &Arc<Session>,
    max_batches: usize,
    per_batch: usize,
    delay_ms: u64,
) -> PhaseReport {
    let started = Instant::now();
    let semaphore = Arc::new(Semaphore::new(per_batch));
    let mut total_requests = 0usize;
    let mut successes = 0usize;
    let mut first_block_at: Option<usize> = None;
    let mut block_type = "none".to_string();
    let mut batches_run = 0usize;

    println!();
    println!("======================================================");
    println!(
        "== ENGINE: {engine:?} — STRATEGY: PARALLEL (RAW backend, no router, PACING UNLOCKED) =="
    );
    println!("======================================================");
    println!(
        "max_batches={max_batches} queries_per_batch={per_batch} delay_between_batches={delay_ms}ms"
    );
    println!(
        "fan-out mirrors batch.rs: JoinSet + shared semaphore; STOP at first batch where ALL queries block"
    );

    'outer: for batch in 1..=max_batches {
        batches_run = batch;
        let base = (batch - 1) * per_batch;
        let queries: Vec<String> = (0..per_batch)
            .map(|i| QUERIES[(base + i) % QUERIES.len()].to_string())
            .collect();
        println!(
            "--- batch {batch}: {per_batch} concurrent queries {:?} ---",
            queries
        );

        let batch_start = Instant::now();
        let mut join_set: JoinSet<(String, String, u64)> = JoinSet::new();
        for q in &queries {
            let session = Arc::clone(session);
            let semaphore = Arc::clone(&semaphore);
            let q = q.clone();
            join_set.spawn(async move {
                // Acquire a permit BEFORE searching; OwnedSemaphorePermit
                // auto-releases on drop, bounding concurrent CDP tabs exactly
                // like batch.rs's search_single.
                let _permit = semaphore.acquire_owned().await.expect("semaphore closed");
                let req_start = Instant::now();
                // Backend built per task from the shared session Arc (no
                // router, no pacing gates); Bing is stateless plain HTTP.
                let result = match engine {
                    SearchEngine::Google => {
                        GoogleBackend::new(Arc::clone(&session))
                            .search(&q, 10, &SearchOptions::default())
                            .await
                    }
                    SearchEngine::Brave => {
                        BraveBackend::new(Arc::clone(&session))
                            .search(&q, 10, &SearchOptions::default())
                            .await
                    }
                    SearchEngine::Bing => {
                        BingBackend::new()
                            .search(&q, 10, &SearchOptions::default())
                            .await
                    }
                    other => {
                        return (
                            q,
                            format!("block(unsupported_engine:{other:?})"),
                            req_start.elapsed().as_millis() as u64,
                        );
                    }
                };
                (q, classify(result), req_start.elapsed().as_millis() as u64)
            });
        }

        let mut batch_outcomes: Vec<(String, String, u64)> = Vec::new();
        while let Some(joined) = join_set.join_next().await {
            match joined {
                Ok(triple) => batch_outcomes.push(triple),
                Err(join_err) => {
                    // A panicked task counts as a block for this query.
                    batch_outcomes.push((
                        "?task".to_string(),
                        format!("block(join_error:{join_err})"),
                        0,
                    ));
                }
            }
        }
        let batch_ms = batch_start.elapsed().as_millis() as u64;

        for (q, outcome, ms) in &batch_outcomes {
            total_requests += 1;
            if !is_block(outcome) {
                successes += 1;
            }
            println!("    {:32.32} -> {outcome} ({ms} ms)", q);
        }
        let blocked = batch_outcomes
            .iter()
            .filter(|(_, o, _)| is_block(o))
            .count();
        println!("    batch {batch} took {batch_ms} ms: {blocked}/{per_batch} queries blocked");

        if blocked == per_batch {
            if first_block_at.is_none() {
                first_block_at = Some(batch);
            }
            block_type = summarize_blocks(&batch_outcomes);
            println!(
                "!!! FULL-BATCH BLOCK at batch {batch} ({block_type}) — stopping parallel phase"
            );
            break 'outer;
        }

        if batch < max_batches {
            println!("    ... sleeping {} ms before next batch", delay_ms);
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        }
    }

    let recovery = run_recovery_probe(engine, session, first_block_at).await;

    finalize_report(
        engine,
        Strategy::Parallel,
        batches_run,
        total_requests,
        successes,
        first_block_at,
        block_type,
        recovery,
        started,
        max_batches,
    )
}

/// HARVEST phase: loop `harvest()` with the engine pinned (router-mediated;
/// the router politely waits out the engine's minimum interval per call),
/// one distinct query per iteration, `follow_top_n` followed, `delay_ms`
/// between iterations. 0 harvested results = block signal (harvest swallows
/// engine search errors to empty at orchestrator/search.rs:95-98). Stops at
/// the first block, then runs a recovery probe.
async fn run_harvest_phase(
    engine: SearchEngine,
    session: &Arc<Session>,
    max_iterations: usize,
    follow_top_n: usize,
    delay_ms: u64,
) -> PhaseReport {
    let started = Instant::now();
    let mut total_requests = 0usize;
    let mut successes = 0usize;
    let mut first_block_at: Option<usize> = None;
    let mut block_type = "none".to_string();
    let mut iterations_run = 0usize;

    println!();
    println!("======================================================");
    println!(
        "== ENGINE: {engine:?} — STRATEGY: HARVEST (router pinned engine, 1 query/iteration) =="
    );
    println!("======================================================");
    println!(
        "max_iterations={max_iterations} follow_top_n={follow_top_n} max_chars={HARVEST_MAX_CHARS} delay_between={delay_ms}ms"
    );
    println!(
        "0 harvested results (search errors swallowed to empty) = block signal; STOP at first block"
    );

    'outer: for iteration in 1..=max_iterations {
        iterations_run = iteration;
        let query = QUERIES[(iteration - 1) % QUERIES.len()];
        let req = BatchHarvestRequest {
            queries: vec![query.to_string()],
            rank_by: RankStrategy::SerpOrder,
            follow_top_n,
            extract_params: ExtractParams {
                offset: 0,
                max_chars: HARVEST_MAX_CHARS,
            },
            reputation: None,
            engine: Some(engine),
        };
        println!("--- harvest iteration {iteration}: \"{query}\" (engine pinned {engine:?}) ---");

        let iter_start = Instant::now();
        total_requests += 1;
        let (harvested, summary) = match harvest(Arc::clone(session), req).await {
            Ok(ok) => ok,
            Err(e) => {
                // The search phase never returns Err on engine failure
                // (swallowed to empty), so a CdpError here is transport-level.
                println!(
                    "    => block(cdp_error) — {e} ({} ms)",
                    iter_start.elapsed().as_millis()
                );
                if first_block_at.is_none() {
                    first_block_at = Some(iteration);
                }
                block_type = format!("cdp_error: {e}");
                break 'outer;
            }
        };
        let elapsed_ms = iter_start.elapsed().as_millis() as u64;

        if harvested.is_empty() {
            println!("    => block(empty_harvest: 0 results) ({elapsed_ms} ms)");
            if first_block_at.is_none() {
                first_block_at = Some(iteration);
            }
            block_type = "empty_harvest (engine search errors swallowed)".to_string();
            break 'outer;
        }

        successes += 1;
        let ok_bodies = harvested
            .iter()
            .filter(|h| matches!(h.body_status, gthings_search::BodyStatus::Ok))
            .count();
        println!(
            "    => success({} results, {ok_bodies} bodies ok, summary.total_results={}) ({elapsed_ms} ms)",
            harvested.len(),
            summary.total_results
        );

        if iteration < max_iterations {
            println!("    ... sleeping {} ms before next harvest", delay_ms);
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        }
    }

    let recovery = run_recovery_probe(engine, session, first_block_at).await;

    finalize_report(
        engine,
        Strategy::Harvest,
        iterations_run,
        total_requests,
        successes,
        first_block_at,
        block_type,
        recovery,
        started,
        max_iterations,
    )
}

#[tokio::test]
#[ignore = "requires live Chrome on 127.0.0.1:9222 and hits live engines; run with --ignored"]
async fn rpm_pacing_unlocked_parallel_and_harvest() {
    let max_n = env_usize("GTHINGS_BENCH_N", 8);
    let per_batch = env_usize("GTHINGS_BENCH_PARALLEL", DEFAULT_PARALLEL_PER_BATCH);
    let follow_top_n = env_usize("GTHINGS_BENCH_FOLLOW_TOP", DEFAULT_HARVEST_FOLLOW_TOP_N);
    let delay_ms = env_u64("GTHINGS_BENCH_DELAY_MS", 500);
    let port = env_u16("GTHINGS_BENCH_CDP_PORT").unwrap_or(9222);

    let session = Arc::new(connect(port).await);

    println!("== RPM benchmark v2: PACING UNLOCKED — true server-side ceilings ==");
    println!(
        "budget per phase = {max_n} ({} queries/batch in parallel; {} follow_top_n in harvest)",
        per_batch, follow_top_n
    );
    println!("delay between batches/iterations = {delay_ms}ms — no in-process pacing gates");
    println!("Google/Brave share one CDP session (via Arc clone); Bing is stateless HTTP RSS.");
    println!("Per engine: PARALLEL (raw backends, pacing unlocked) then HARVEST (router pinned).");
    println!(
        "Each phase stops at the first block (all-block batch / empty harvest), then probes recovery."
    );
    println!();

    let mut reports: Vec<PhaseReport> = Vec::new();
    for engine in [
        SearchEngine::Google,
        SearchEngine::Brave,
        SearchEngine::Bing,
    ] {
        reports.push(run_parallel_phase(engine, &session, max_n, per_batch, delay_ms).await);
        reports.push(run_harvest_phase(engine, &session, max_n, follow_top_n, delay_ms).await);
    }

    println!();
    println!("======================================================");
    println!("== TRUE CEILINGS WITH PACING UNLOCKED (side-by-side) ==");
    println!("======================================================");
    println!(
        "{:<8} {:<9} {:>9} {:>9} {:>13} {:>14} {:>12}",
        "engine", "strategy", "requests", "successes", "first_block", "block_type", "req/min"
    );
    println!("{:─<110}", "");
    for r in &reports {
        let first = match r.first_block_at {
            Some(n) => format!("{} #{n}", r.strategy.as_str()),
            None => "none".to_string(),
        };
        println!(
            "{:<8} {:<9} {:>9} {:>9} {:>13} {:>14} {:>12.1}",
            r.engine.as_str(),
            r.strategy.as_str(),
            r.total_requests,
            r.successes,
            first,
            r.block_type,
            r.implied_rpm
        );
        println!("  recovery: {}", r.recovery);
    }
    println!("{:─<110}", "");
    println!(
        "req/min = requests actually sent per minute at the point of stopping (block or budget end)"
    );
    println!("parallel = raw backends, no router, pacing unlocked — the TRUE server-side ceiling");
    println!(
        "harvest  = router pinned engine — politely throttled to the engine's min interval per call"
    );
    println!(
        "'none' first_block = no block within the budget; the engine tolerates at least this rate."
    );
    println!("======================================================");
}
