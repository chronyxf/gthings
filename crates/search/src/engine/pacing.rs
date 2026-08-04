//! In-memory per-engine pacing state.
//!
//! The search router enforces a minimum interval between queries per engine.
//! gthings runs as ONE long-lived process (or is embedded as a library), so
//! engine pacing is fully in-memory — there is no disk persistence. This store
//! tracks the last-call timestamp per engine and any rate-limit/captcha
//! cooldowns, all held in memory for the lifetime of the process.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use super::SearchEngine;

/// Process-wide shared pacing store.
///
/// gthings runs as ONE long-lived process (or is embedded as a library), so
/// engine pacing must persist across [`SearchRouter`](crate::engine::router::SearchRouter)
/// instances built over the process's lifetime. Every router constructed via
/// [`SearchRouter::new`](crate::engine::router::SearchRouter::new) shares this
/// single store, so minimum-interval pacing and cooldowns survive the
/// orchestrator rebuilding a fresh router per harvest call.
static GLOBAL_PACING: OnceLock<Arc<Mutex<PacingStore>>> = OnceLock::new();

/// Accessor for the process-wide shared pacing store, initializing it on first
/// use. All routers built with [`SearchRouter::new`] share this store.
pub(crate) fn global_pacing() -> &'static Arc<Mutex<PacingStore>> {
    GLOBAL_PACING.get_or_init(|| Arc::new(Mutex::new(PacingStore::new())))
}

/// Last-call timestamps and cooldowns per engine, held in memory.
///
/// Keys are [`SearchEngine::as_str`] identifiers; values are unix timestamps
/// in milliseconds since the epoch.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct PacingStore {
    /// engine identifier → last-call unix millis.
    last_calls: HashMap<String, u64>,
    /// engine identifier → unix millis until which the engine is blocked.
    cooldowns: HashMap<String, u64>,
}

impl PacingStore {
    /// Create an empty in-memory store (no last-call timestamps, no
    /// cooldowns).
    pub(super) fn new() -> Self {
        Self {
            last_calls: HashMap::new(),
            cooldowns: HashMap::new(),
        }
    }

    /// Unix timestamp (milliseconds) of the last recorded call to `engine`.
    pub(super) fn last_call_ms(&self, engine: SearchEngine) -> Option<u64> {
        self.last_calls.get(engine.as_str()).copied()
    }

    /// Unix timestamp (milliseconds) until which `engine` is cooled down
    /// (rate-limit/captcha block), or `None` when it is not blocked.
    pub(super) fn cooldown_until_ms(&self, engine: SearchEngine) -> Option<u64> {
        self.cooldowns.get(engine.as_str()).copied()
    }

    /// Record that `engine` was dispatched at `now_ms` (unix milliseconds).
    pub(super) fn record(&mut self, engine: SearchEngine, now_ms: u64) {
        self.last_calls.insert(engine.as_str().to_string(), now_ms);
    }

    /// Record a cooldown block for `engine` until `until_ms` (unix
    /// milliseconds).
    pub(super) fn record_cooldown(&mut self, engine: SearchEngine, until_ms: u64) {
        self.cooldowns.insert(engine.as_str().to_string(), until_ms);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_returns_empty_store() {
        let store = PacingStore::new();
        assert!(store.last_calls.is_empty());
        assert!(store.cooldowns.is_empty());
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
}
