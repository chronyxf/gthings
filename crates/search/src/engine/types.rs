//! Core engine types: the [`SearchEngine`] enumeration, normalized result and
//! error types, routing choices ([`EngineChoice`], [`EngineMode`]), and the
//! shared env-var resolution helper ([`env_var_from`]).

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Maximum concurrent CDP tabs opened across a batch, harvest search, or
/// harvest follow wave.
pub(crate) const MAX_CONCURRENT_TABS: usize = 4;

/// Per-operation timeout for batch search, harvest search/follow, and their
/// semaphore acquire bounds.
pub(crate) const OP_TIMEOUT: Duration = Duration::from_secs(30);

/// Supported search engines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchEngine {
    Brave,
    Bing,
    Google,
    /// Brave Search API (paid subscription backend; see [`crate::engine::api::brave`]).
    #[serde(rename = "brave_api")]
    BraveApi,
    /// Tavily Search API (paid backend; see [`crate::engine::api::tavily`]).
    Tavily,
}

impl SearchEngine {
    /// Whether this engine must run inside a browser (CDP).
    ///
    /// Bing is scraped over plain HTTP; Brave and Google
    /// require a real browser to avoid blocking (Brave rate-limits plain
    /// HTTP aggressively: 429s with ~60s pacing). The paid API backends
    /// (Brave API, Tavily) are plain HTTP too.
    pub fn requires_browser(&self) -> bool {
        match self {
            SearchEngine::Brave => true,
            SearchEngine::Bing => false,
            SearchEngine::Google => true,
            SearchEngine::BraveApi | SearchEngine::Tavily => false,
        }
    }

    /// Short stable identifier used in config, logs, and provenance.
    pub fn as_str(&self) -> &'static str {
        match self {
            SearchEngine::Brave => "brave",
            SearchEngine::Bing => "bing",
            SearchEngine::Google => "google",
            SearchEngine::BraveApi => "brave_api",
            SearchEngine::Tavily => "tavily",
        }
    }

    /// Whether this engine is a paid API backend, gated by the aggregate
    /// quota counter (see [`crate::engine::pacing`]).
    pub fn is_paid(&self) -> bool {
        matches!(self, SearchEngine::BraveApi | SearchEngine::Tavily)
    }

    /// Free (no-subscription) engine preference order: Google first for
    /// result quality, then Brave (CDP-rendered), then Bing as the last
    /// choice (plain HTTP). Google and Brave are skipped by the router when
    /// no CDP session exists (see [`Self::requires_browser`]), so listing
    /// them first is safe — it falls back to Bing.
    pub const FREE_PRIORITY: [SearchEngine; 3] = [
        SearchEngine::Google,
        SearchEngine::Brave,
        SearchEngine::Bing,
    ];

    /// Paid API backend preference order.
    pub const API_PRIORITY: [SearchEngine; 2] = [SearchEngine::BraveApi, SearchEngine::Tavily];

    /// Hybrid: free engines first, paid API backends as fallback.
    pub const HYBRID_PRIORITY: [SearchEngine; 5] = [
        SearchEngine::Google,
        SearchEngine::Brave,
        SearchEngine::Bing,
        SearchEngine::BraveApi,
        SearchEngine::Tavily,
    ];
}

/// A single organic search result from any engine, normalized across engines.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineSearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub position: usize,
    pub engine: SearchEngine,
    /// Relevance score (0.0–1.0) supplied by the backend, or 0.0 when the
    /// backend exposes none (scrape backends).
    #[serde(default)]
    pub score: f64,
    /// Publication date when the backend exposes one (e.g. Brave's relative
    /// date prefix, Tavily's `published_date`); `None` otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_date: Option<String>,
    /// Favicon URL when the backend exposes one (e.g. Brave scrape); `None`
    /// otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub favicon: Option<String>,
}

/// Optional per-search tuning threaded to engine backends.
///
/// Both fields are optional: `None` means "use the engine's default". Only
/// the paid API backends (Brave API, Tavily) consume them today; the scrape
/// backends ignore them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchOptions {
    /// Recency filter (e.g. `"day"`/`"week"`/`"month"`/`"year"` or an ISO
    /// date). Passed to Tavily's `freshness` and Brave's `freshness` param.
    pub freshness: Option<String>,
    /// Search depth for engines that support it (`"basic"`/`"advanced"`).
    /// Consumed by Tavily's `search_depth`.
    pub search_depth: Option<String>,
}

/// Errors raised by search engine backends.
#[derive(Debug, thiserror::Error)]
pub enum SearchEngineError {
    #[error("{engine:?} rate limited: {detail}")]
    RateLimited {
        engine: SearchEngine,
        detail: String,
        /// Milliseconds to wait before retrying, when the backend supplied a
        /// `Retry-After` header; `None` when it did not.
        retry_after_ms: Option<u64>,
    },
    #[error("{engine:?} captcha/block page: {detail}")]
    Captcha {
        engine: SearchEngine,
        detail: String,
    },
    #[error("{engine:?} aggregate API quota exceeded: {detail}")]
    /// The aggregate paid-API quota was exhausted. Consumers map this to
    /// HTTP 429 with the canonical `quota-exceeded` error code
    /// ([`gthings_common::taxonomy::ErrorCode::QuotaExceeded`]).
    QuotaExceeded {
        engine: SearchEngine,
        detail: String,
    },
    #[error("{engine:?} network error: {detail}")]
    Network {
        engine: SearchEngine,
        detail: String,
    },
    #[error("{engine:?} parse error: {detail}")]
    Parse {
        engine: SearchEngine,
        detail: String,
    },
    #[error("{engine:?} unavailable: {detail}")]
    Unavailable {
        engine: SearchEngine,
        detail: String,
    },
    #[error("all search engines failed: {0:?}")]
    AllEnginesFailed(Vec<SearchEngineError>),
}

/// How the orchestrator should pick an engine for a query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EngineChoice {
    /// Try engines in the active [`EngineMode`] priority order.
    Auto,
    /// Use exactly this engine.
    Pin(SearchEngine),
}

/// Hybrid engine routing mode: which engines a search may dispatch.
///
/// `engine=free|hybrid|api` (default `hybrid`), configurable via the
/// `GTHINGS_ENGINE_MODE` env var. `free` preserves the pre-hybrid behavior:
/// only the free engines (Google, Brave scrape, Bing) are used. `hybrid`
/// tries the free engines first and falls back to the paid backends (Brave
/// API, Tavily) when they fail or are blocked. `api` uses only the paid
/// backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EngineMode {
    /// Free engines only (Google, Brave, Bing) — no paid spend.
    Free,
    /// Free engines first; paid backends (Brave API, Tavily) as fallback.
    #[default]
    Hybrid,
    /// Paid API backends only (Brave API, Tavily).
    Api,
}

impl EngineMode {
    /// Short stable identifier used in config and logs.
    pub fn as_str(self) -> &'static str {
        match self {
            EngineMode::Free => "free",
            EngineMode::Hybrid => "hybrid",
            EngineMode::Api => "api",
        }
    }

    /// Engine preference order for this mode.
    pub fn priority(self) -> &'static [SearchEngine] {
        match self {
            EngineMode::Free => &SearchEngine::FREE_PRIORITY,
            EngineMode::Hybrid => &SearchEngine::HYBRID_PRIORITY,
            EngineMode::Api => &SearchEngine::API_PRIORITY,
        }
    }

    /// Parse a mode string (`free`|`hybrid`|`api`, case-insensitive),
    /// defaulting to [`EngineMode::Hybrid`] when unset or invalid.
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "free" => EngineMode::Free,
            "api" => EngineMode::Api,
            _ => EngineMode::Hybrid,
        }
    }

    /// Resolve the mode from the `GTHINGS_ENGINE_MODE` env var
    /// (`engine=free|hybrid|api`), defaulting to [`EngineMode::Hybrid`].
    pub fn from_env() -> Self {
        Self::parse(&std::env::var("GTHINGS_ENGINE_MODE").unwrap_or_default())
    }
}

/// Resolve the first non-empty value among `names`, in order, from `vars`.
///
/// Pure helper (injectable env) shared by the paid API backends
/// ([`crate::engine::api::brave`], [`crate::engine::api::tavily`]) for
/// subscription-key resolution, so key precedence is unit-testable without
/// mutating process environment variables.
pub(crate) fn env_var_from(
    names: &[&str],
    vars: impl IntoIterator<Item = (String, String)>,
) -> Option<String> {
    let vars: Vec<(String, String)> = vars.into_iter().collect();
    for name in names {
        let found = vars.iter().find(|entry| entry.0.as_str() == *name);
        if let Some(entry) = found {
            let value = entry.1.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}
