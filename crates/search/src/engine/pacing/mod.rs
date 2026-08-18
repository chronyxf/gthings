//! Per-engine pacing state with optional disk persistence.
//!
//! The search router enforces a minimum interval between queries per engine.
//! This store tracks the last-call timestamp per engine, any rate-limit/
//! captcha cooldowns, and the **aggregate paid-API quota counter** (spend
//! across all paid backends).
//!
//! Pacing state is persisted to disk when a directory is configured
//! (`GTHINGS_PACING_DIR`, falling back to `GTHINGS_REPUTATION_DIR`), so
//! cooldowns and last-call timestamps survive a daemon restart. Without a
//! configured directory the store is fully in-memory (unchanged behavior).
//!
//! The persistence machinery (serializable snapshot, background writer
//! thread, atomic write path) lives in the [`persist`] submodule.

mod persist;

use std::collections::HashMap;
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use self::persist::{PersistWriter, PersistedState};
use super::SearchEngine;

/// Default aggregate paid-API quota limit (`GTHINGS_API_QUOTA_LIMIT`).
pub const DEFAULT_API_QUOTA_LIMIT: u64 = 2000;

/// Default minimum interval between Brave queries, in unix-millisecond scale
/// (the fallback when `GTHINGS_BRAVE_MIN_INTERVAL_MS` is unset or unparseable).
///
/// Was 60s back when Brave was scraped over plain HTTP — the live server
/// throttled to ~1 query per 35-42s (429s under sustained load), so 60s gave
/// a comfortable margin. The Brave backend is now CDP-based (renders
/// search.brave.com in Chrome), and a live benchmark proved 2.5s-spaced
/// sequential searches are tolerated with no block. 5s is the conservative
/// default: above the proven 2.5s floor while leaving headroom for Brave's
/// heuristics at volume. Override via `GTHINGS_BRAVE_MIN_INTERVAL_MS`.
const BRAVE_MIN_INTERVAL_MS: u64 = 5_000;

/// Minimum interval between Bing queries.
const BING_MIN_INTERVAL_MS: u64 = 1_000;

/// Minimum interval between Google queries.
///
/// Google is precious — one public IP — so it is throttled hardest.
const GOOGLE_MIN_INTERVAL_MS: u64 = 6_000;

/// Minimum interval between Brave API queries (paid subscription).
const BRAVE_API_MIN_INTERVAL_MS: u64 = 1_000;

/// Minimum interval between Tavily queries (paid subscription).
const TAVILY_MIN_INTERVAL_MS: u64 = 1_000;

/// Minimum interval between queries for `engine` (the token-bucket refill), in
/// unix-millisecond scale.
///
/// The single source of truth for query budgets: the router
/// ([`super::router::min_interval`](crate::engine::router)) derives its
/// `Duration` from this table, and [`PacingStore::pacing_snapshot`] uses it to
/// compute `retry_after_ms` for healthz.
pub fn min_interval_ms(engine: SearchEngine) -> u64 {
    match engine {
        SearchEngine::Brave => brave_min_interval_ms(),
        SearchEngine::Bing => BING_MIN_INTERVAL_MS,
        SearchEngine::Google => GOOGLE_MIN_INTERVAL_MS,
        SearchEngine::BraveApi => BRAVE_API_MIN_INTERVAL_MS,
        SearchEngine::Tavily => TAVILY_MIN_INTERVAL_MS,
    }
}

/// Remaining milliseconds until `engine`'s minimum interval has elapsed since
/// its last recorded call, or `None` when it may be dispatched right away.
///
/// The single implementation of the pacing decision: [`PacingStore`] uses it
/// for `pacing_snapshot` (kept here so the store stays self-contained), and
/// the router delegates to it via
/// [`crate::engine::router::select::pacing_remaining_ms`]. `last_call_ms` is
/// the persisted last-call timestamp, `min_interval_ms` the engine's minimum
/// interval, `now_ms` the current unix millis.
pub(crate) fn remaining_ms(
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

/// Env var selecting the pacing-persistence directory. Falls back to
/// `GTHINGS_REPUTATION_DIR`; when neither is set, pacing stays in-memory.
const PACING_DIR_ENV: &str = "GTHINGS_PACING_DIR";

/// Env var consulted as a fallback persistence directory.
const REPUTATION_DIR_ENV: &str = "GTHINGS_REPUTATION_DIR";

/// Env var overriding the aggregate paid-API quota limit.
const QUOTA_LIMIT_ENV: &str = "GTHINGS_API_QUOTA_LIMIT";

/// Env var overriding the minimum interval between Brave queries (ms).
const BRAVE_MIN_INTERVAL_ENV: &str = "GTHINGS_BRAVE_MIN_INTERVAL_MS";

/// Filename inside the pacing dir holding the serialized pacing state.
const PACING_FILE: &str = "pacing.json";

/// Process-wide shared pacing store.
///
/// Every router constructed via
/// [`SearchRouter::new`](crate::engine::router::SearchRouter::new) shares this
/// single store, so minimum-interval pacing, cooldowns, and the aggregate
/// quota counter survive router rebuilds. When a pacing directory is
/// configured, the store is loaded from disk on first access and persisted on
/// every mutation.
static GLOBAL_PACING: OnceLock<Arc<Mutex<PacingStore>>> = OnceLock::new();

/// Accessor for the process-wide shared pacing store, initializing it on first
/// use (loading persisted state when a pacing directory is configured).
///
/// Healthz consumes it via [`PacingStore::pacing_snapshot`] for pacing
/// visibility.
pub fn global_pacing() -> &'static Arc<Mutex<PacingStore>> {
    GLOBAL_PACING
        .get_or_init(|| Arc::new(Mutex::new(PacingStore::load_from_env().unwrap_or_default())))
}

/// A point-in-time view of one engine's pacing state (healthz §9 pacing
/// visibility).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PacingSnapshot {
    /// The engine this snapshot describes.
    pub engine: SearchEngine,
    /// Unix millis of the engine's last dispatched query, or `None` if never
    /// dispatched in this store.
    pub last_call_ms: Option<u64>,
    /// Unix millis until which the engine is blocked, or `None` when not in a
    /// cooldown block.
    pub cooldown_until_ms: Option<u64>,
    /// Remaining milliseconds until the engine may be dispatched again — the
    /// max of any active cooldown and its minimum interval since the last
    /// call. `0` means the engine is dispatchable right now.
    pub retry_after_ms: u64,
}

/// Last-call timestamps, cooldowns, and aggregate quota spend per engine,
/// optionally persisted to disk.
///
/// Keys are [`SearchEngine::as_str`] identifiers; values are unix timestamps
/// in milliseconds since the epoch.
#[derive(Debug, Default)]
pub struct PacingStore {
    /// engine identifier → last-call unix millis.
    last_calls: HashMap<String, u64>,
    /// engine identifier → unix millis until which the engine is blocked.
    cooldowns: HashMap<String, u64>,
    /// Aggregate spend against the paid-API quota (see [`Self::quota_limit`]).
    api_quota_spend: u64,
    /// Persistence directory; `None` keeps the store in-memory only.
    dir: Option<PathBuf>,
    /// Background writer persisting snapshots off the async executor; `None`
    /// when no directory is configured.
    writer: Option<PersistWriter>,
}

impl PacingStore {
    /// Create an empty in-memory store (no last-call timestamps, no cooldowns,
    /// no disk persistence).
    ///
    /// Production code constructs stores via [`Self::load_from_env`] or
    /// [`Self::load_from_dir`], so this convenience constructor is only used by
    /// tests; gate it on `cfg(test)` to avoid dead-code warnings in the
    /// non-test build.
    #[cfg(test)]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Load the store from `dir`, creating a fresh empty store when the file
    /// is missing or unreadable. `dir` becomes the store's persistence target.
    pub fn load_from_dir(dir: PathBuf) -> Self {
        let mut store = Self {
            dir: Some(dir.clone()),
            writer: Some(PersistWriter::spawn(dir)),
            ..Self::default()
        };
        let path = store.path();
        if let Ok(contents) = std::fs::read_to_string(path) {
            if let Ok(state) = serde_json::from_str::<PersistedState>(&contents) {
                store.last_calls = state.last_calls;
                store.cooldowns = state.cooldowns;
                store.api_quota_spend = state.api_quota_spend;
            }
        }
        store
    }

    /// Resolve the pacing directory from the process environment
    /// (`GTHINGS_PACING_DIR`, else `GTHINGS_REPUTATION_DIR`) and load the
    /// store from it; `None` when no directory is configured.
    pub fn load_from_env() -> Option<Self> {
        pacing_dir_from_env(std::env::vars()).map(Self::load_from_dir)
    }

    /// Unix timestamp (milliseconds) of the last recorded call to `engine`.
    pub fn last_call_ms(&self, engine: SearchEngine) -> Option<u64> {
        self.last_calls.get(engine.as_str()).copied()
    }

    /// Unix timestamp (milliseconds) until which `engine` is cooled down
    /// (rate-limit/captcha block), or `None` when it is not blocked.
    pub fn cooldown_until_ms(&self, engine: SearchEngine) -> Option<u64> {
        self.cooldowns.get(engine.as_str()).copied()
    }

    /// Record that `engine` was dispatched at `now_ms` (unix milliseconds).
    pub fn record(&mut self, engine: SearchEngine, now_ms: u64) {
        self.last_calls.insert(engine.as_str().to_string(), now_ms);
        self.save();
    }

    /// Record a cooldown block for `engine` until `until_ms` (unix
    /// milliseconds).
    pub fn record_cooldown(&mut self, engine: SearchEngine, until_ms: u64) {
        self.cooldowns.insert(engine.as_str().to_string(), until_ms);
        self.save();
    }

    /// Count one paid-API dispatch against the aggregate quota. Free-engine
    /// dispatches never reach here.
    pub fn bump_quota(&mut self) {
        self.api_quota_spend = self.api_quota_spend.saturating_add(1);
        self.save();
    }

    /// Current aggregate paid-API spend.
    pub fn quota_spend(&self) -> u64 {
        self.api_quota_spend
    }

    /// The aggregate paid-API quota limit: `GTHINGS_API_QUOTA_LIMIT`
    /// (default [`DEFAULT_API_QUOTA_LIMIT`]).
    pub fn quota_limit(&self) -> u64 {
        api_quota_limit()
    }

    /// Whether the aggregate paid-API quota has been exhausted.
    pub fn quota_exceeded(&self) -> bool {
        self.api_quota_spend >= api_quota_limit()
    }

    /// Snapshot of every engine's pacing state for healthz (pacing
    /// visibility).
    ///
    /// Returns one [`PacingSnapshot`] per engine in hybrid priority order
    /// (free engines first, then paid backends). `now_ms` is the current unix
    /// millis; `retry_after_ms` is computed as the max of any remaining
    /// cooldown and the engine's remaining minimum interval since its last
    /// call.
    pub fn pacing_snapshot(&self, now_ms: u64) -> Vec<PacingSnapshot> {
        SearchEngine::HYBRID_PRIORITY
            .iter()
            .map(|&engine| {
                let last_call_ms = self.last_call_ms(engine);
                let cooldown_until_ms = self.cooldown_until_ms(engine);
                let cooldown_remaining = cooldown_until_ms
                    .map(|until| until.saturating_sub(now_ms))
                    .unwrap_or(0);
                let interval_remaining =
                    remaining_ms(last_call_ms, min_interval_ms(engine), now_ms).unwrap_or(0);
                PacingSnapshot {
                    engine,
                    last_call_ms,
                    cooldown_until_ms,
                    retry_after_ms: cooldown_remaining.max(interval_remaining),
                }
            })
            .collect()
    }

    /// Absolute path of the persistence file (`{dir}/pacing.json`).
    fn path(&self) -> PathBuf {
        self.dir
            .as_ref()
            .map(|d| d.join(PACING_FILE))
            .unwrap_or_default()
    }

    /// Persist the current state to disk. No-op when no directory is
    /// configured. The snapshot is handed to a dedicated background writer
    /// thread (a non-blocking channel send), so file I/O never happens on the
    /// async executor or under the pacing lock. Writes are atomic (temp file +
    /// rename); I/O failures are logged and swallowed — pacing must never fail
    /// a search.
    fn save(&self) {
        let Some(writer) = &self.writer else {
            return;
        };
        let state = PersistedState {
            last_calls: self.last_calls.clone(),
            cooldowns: self.cooldowns.clone(),
            api_quota_spend: self.api_quota_spend,
        };
        writer.enqueue(state);
    }

    /// Block until every enqueued snapshot has reached the disk.
    ///
    /// Test-only: the background writer makes persistence asynchronous from the
    /// caller's perspective, so tests flush before reloading the file.
    #[cfg(test)]
    fn flush(&self) {
        if let Some(writer) = &self.writer {
            writer.flush();
        }
    }
}

/// Resolve the pacing directory from an injectable `(key, value)` env iterable:
/// `GTHINGS_PACING_DIR` wins over `GTHINGS_REPUTATION_DIR`; empty values are
/// skipped.
fn pacing_dir_from_env(vars: impl IntoIterator<Item = (String, String)>) -> Option<PathBuf> {
    super::env_var_from(&[PACING_DIR_ENV, REPUTATION_DIR_ENV], vars).map(PathBuf::from)
}

/// The aggregate paid-API quota limit: `GTHINGS_API_QUOTA_LIMIT` (parsed as a
/// non-negative integer), defaulting to [`DEFAULT_API_QUOTA_LIMIT`].
fn api_quota_limit() -> u64 {
    std::env::var(QUOTA_LIMIT_ENV)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(DEFAULT_API_QUOTA_LIMIT)
}

/// The effective minimum interval between Brave queries (ms):
/// `GTHINGS_BRAVE_MIN_INTERVAL_MS` (parsed as a non-negative integer),
/// defaulting to [`BRAVE_MIN_INTERVAL_MS`].
fn brave_min_interval_ms() -> u64 {
    brave_min_interval_from_env(std::env::vars())
}

/// Resolve the Brave minimum interval from an injectable `(key, value)` env
/// iterable (mirrors [`pacing_dir_from_env`]): `GTHINGS_BRAVE_MIN_INTERVAL_MS`
/// parsed as a non-negative integer, defaulting to [`BRAVE_MIN_INTERVAL_MS`];
/// unset, empty, or unparseable values fall back to the default.
fn brave_min_interval_from_env(vars: impl IntoIterator<Item = (String, String)>) -> u64 {
    super::env_var_from(&[BRAVE_MIN_INTERVAL_ENV], vars)
        .and_then(|v| v.parse().ok())
        .unwrap_or(BRAVE_MIN_INTERVAL_MS)
}

/// Whether `path` looks like a temporary write target of [`PacingStore`] (used
/// only in tests to assert atomic writes).
#[cfg(test)]
fn is_tmp_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.ends_with(".tmp"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_returns_empty_store() {
        let store = PacingStore::new();
        assert!(store.last_calls.is_empty());
        assert!(store.cooldowns.is_empty());
        assert_eq!(store.quota_spend(), 0);
    }

    #[test]
    fn record_sets_last_call() {
        let mut store = PacingStore::new();
        assert_eq!(store.last_call_ms(SearchEngine::Brave), None);
        store.record(SearchEngine::Brave, 1_700_000_000_000);
        assert_eq!(
            store.last_call_ms(SearchEngine::Brave),
            Some(1_700_000_000_000)
        );
        assert_eq!(store.last_call_ms(SearchEngine::Google), None);
    }

    #[test]
    fn record_cooldown_sets_cooldown() {
        let mut store = PacingStore::new();
        assert_eq!(store.cooldown_until_ms(SearchEngine::Brave), None);
        store.record_cooldown(SearchEngine::Brave, 1_700_000_360_000);
        assert_eq!(
            store.cooldown_until_ms(SearchEngine::Brave),
            Some(1_700_000_360_000)
        );
        assert_eq!(store.cooldown_until_ms(SearchEngine::Google), None);
    }

    #[test]
    fn last_call_ms_none_when_unrecorded() {
        let store = PacingStore::new();
        assert_eq!(store.last_call_ms(SearchEngine::Bing), None);
        assert_eq!(store.last_call_ms(SearchEngine::Google), None);
    }

    #[test]
    fn cooldown_until_ms_none_when_unset() {
        let store = PacingStore::new();
        assert_eq!(store.cooldown_until_ms(SearchEngine::Bing), None);
        assert_eq!(store.cooldown_until_ms(SearchEngine::Google), None);
    }

    #[test]
    fn quota_bumps_saturate_and_report() {
        let mut store = PacingStore::new();
        assert_eq!(store.quota_spend(), 0);
        store.bump_quota();
        store.bump_quota();
        assert_eq!(store.quota_spend(), 2);
        assert_eq!(store.quota_limit(), DEFAULT_API_QUOTA_LIMIT);
        assert!(!store.quota_exceeded());
    }

    #[test]
    fn pacing_dir_prefers_pacing_over_reputation() {
        let vars = vec![
            ("GTHINGS_PACING_DIR".to_string(), "/data/pacing".to_string()),
            (
                "GTHINGS_REPUTATION_DIR".to_string(),
                "/data/reputation".to_string(),
            ),
        ];
        assert_eq!(
            pacing_dir_from_env(vars),
            Some(PathBuf::from("/data/pacing"))
        );

        let vars = vec![(
            "GTHINGS_REPUTATION_DIR".to_string(),
            "/data/reputation".to_string(),
        )];
        assert_eq!(
            pacing_dir_from_env(vars),
            Some(PathBuf::from("/data/reputation"))
        );

        let vars = vec![("GTHINGS_PACING_DIR".to_string(), "  ".to_string())];
        assert_eq!(pacing_dir_from_env(vars), None);

        assert_eq!(pacing_dir_from_env(Vec::<(String, String)>::new()), None);
    }

    #[test]
    fn quota_limit_parses_env_with_default() {
        // Env-dependent; run with the process default (no env mutation).
        assert!(api_quota_limit() >= DEFAULT_API_QUOTA_LIMIT);
    }

    #[test]
    fn brave_min_interval_defaults_to_five_seconds() {
        // Pins the conservative CDP-era default (benchmark floor was 2.5s).
        assert_eq!(BRAVE_MIN_INTERVAL_MS, 5_000);
        // Env-dependent; run with the process default (no env mutation).
        assert_eq!(min_interval_ms(SearchEngine::Brave), BRAVE_MIN_INTERVAL_MS);
    }

    #[test]
    fn brave_min_interval_env_override_is_deterministic() {
        // Injectable env (mirrors pacing_dir_prefers_pacing_over_reputation):
        // a valid override wins, and the legacy HTTP-scraper 60s interval can
        // be restored via the env var.
        let vars = vec![(
            "GTHINGS_BRAVE_MIN_INTERVAL_MS".to_string(),
            "2500".to_string(),
        )];
        assert_eq!(brave_min_interval_from_env(vars), 2_500);

        let vars = vec![(
            "GTHINGS_BRAVE_MIN_INTERVAL_MS".to_string(),
            "60000".to_string(),
        )];
        assert_eq!(brave_min_interval_from_env(vars), 60_000);

        // Empty and unparseable values fall back to the default; unset too.
        let vars = vec![(
            "GTHINGS_BRAVE_MIN_INTERVAL_MS".to_string(),
            "  ".to_string(),
        )];
        assert_eq!(brave_min_interval_from_env(vars), BRAVE_MIN_INTERVAL_MS);

        let vars = vec![(
            "GTHINGS_BRAVE_MIN_INTERVAL_MS".to_string(),
            "not-a-number".to_string(),
        )];
        assert_eq!(brave_min_interval_from_env(vars), BRAVE_MIN_INTERVAL_MS);

        assert_eq!(
            brave_min_interval_from_env(Vec::<(String, String)>::new()),
            BRAVE_MIN_INTERVAL_MS
        );
    }

    #[test]
    fn pacing_snapshot_reports_per_engine_shape() {
        // Every engine in hybrid priority order gets one snapshot entry with
        // the expected {last_call_ms, cooldown_until_ms, retry_after_ms} shape.
        let now_ms = 1_700_000_000_000;
        let mut store = PacingStore::new();
        store.record(SearchEngine::Brave, now_ms);
        store.record_cooldown(SearchEngine::Bing, now_ms + 10_000);

        let snapshots = store.pacing_snapshot(now_ms);
        assert_eq!(snapshots.len(), SearchEngine::HYBRID_PRIORITY.len());
        // Snapshots follow hybrid priority order (free engines first, paid last).
        for (snapshot, &engine) in snapshots.iter().zip(SearchEngine::HYBRID_PRIORITY.iter()) {
            assert_eq!(snapshot.engine, engine);
        }

        let brave = snapshots
            .iter()
            .find(|s| s.engine == SearchEngine::Brave)
            .unwrap();
        assert_eq!(brave.last_call_ms, Some(now_ms));
        assert_eq!(brave.cooldown_until_ms, None);
        assert_eq!(
            brave.retry_after_ms,
            min_interval_ms(SearchEngine::Brave),
            "Brave just dispatched: full interval remaining"
        );

        let bing = snapshots
            .iter()
            .find(|s| s.engine == SearchEngine::Bing)
            .unwrap();
        assert_eq!(bing.last_call_ms, None);
        assert_eq!(bing.cooldown_until_ms, Some(now_ms + 10_000));
        assert_eq!(
            bing.retry_after_ms, 10_000,
            "Bing cooling down: retry blocked by the active cooldown"
        );

        let google = snapshots
            .iter()
            .find(|s| s.engine == SearchEngine::Google)
            .unwrap();
        assert_eq!(google.last_call_ms, None);
        assert_eq!(google.cooldown_until_ms, None);
        assert_eq!(
            google.retry_after_ms, 0,
            "cold engines are dispatchable immediately"
        );
    }
}
