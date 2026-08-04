//! Multi-engine search abstraction.
//!
//! Defines the [`SearchEngine`] enumeration, the [`SearchEngineBackend`]
//! trait implemented by concrete engine backends, shared result/error types,
//! and crate-internal HTTP infrastructure used by the HTTP-based backends
//! (brave, bing).

pub mod bing;
pub mod brave;
pub mod google;
pub mod html;
mod pacing;
pub mod router;
pub mod technique;

use std::sync::OnceLock;

use reqwest::header::{ACCEPT_LANGUAGE, USER_AGENT};
use serde::{Deserialize, Serialize};

/// Supported search engines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SearchEngine {
    #[serde(rename = "ddg")]
    DuckDuckGo,
    Brave,
    Bing,
    Google,
}

impl SearchEngine {
    /// Whether this engine must run inside a browser (CDP).
    ///
    /// DuckDuckGo, Brave, and Bing are scraped over plain HTTP; only Google
    /// requires a real browser to avoid blocking.
    pub fn requires_browser(&self) -> bool {
        match self {
            SearchEngine::DuckDuckGo | SearchEngine::Brave => false,
            SearchEngine::Bing => false,
            SearchEngine::Google => true,
        }
    }

    /// Short stable identifier used in config, logs, and provenance.
    pub fn as_str(&self) -> &'static str {
        match self {
            SearchEngine::DuckDuckGo => "ddg",
            SearchEngine::Brave => "brave",
            SearchEngine::Bing => "bing",
            SearchEngine::Google => "google",
        }
    }

    /// Engine preference order: fastest/least-blocked first.
    /// DDG has been removed (bot detection / TLS fingerprinting).
    pub const PRIORITY: [SearchEngine; 3] = [
        SearchEngine::Brave,
        SearchEngine::Bing,
        SearchEngine::Google,
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
}

/// Errors raised by search engine backends.
#[derive(Debug, thiserror::Error)]
pub enum SearchEngineError {
    #[error("{engine:?} rate limited: {detail}")]
    RateLimited {
        engine: SearchEngine,
        detail: String,
    },
    #[error("{engine:?} captcha/block page: {detail}")]
    Captcha {
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
    /// Try engines in [`SearchEngine::PRIORITY`] order.
    Auto,
    /// Use exactly this engine.
    Pin(SearchEngine),
}

/// Implemented by each concrete engine backend (ddg.rs, brave.rs, ...).
// Trait used statically only (never dyn); futures are Send — see impls in ddg/bing/brave/google.
#[allow(async_fn_in_trait)]
pub trait SearchEngineBackend: Send + Sync {
    /// The engine this backend implements.
    fn name(&self) -> SearchEngine;

    /// Whether this backend needs a browser session (CDP) to search.
    fn requires_browser(&self) -> bool {
        self.name().requires_browser()
    }

    /// Run a search for `query`, returning up to `count` normalized results.
    async fn search(
        &self,
        query: &str,
        count: usize,
    ) -> Result<Vec<EngineSearchResult>, SearchEngineError>;
}

/// Browser-like User-Agent used by HTTP backends to avoid bot detection.
const BROWSER_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36";

/// Shared, lazily-built HTTP client for plain-HTTP backends (ddg, brave).
///
/// Configured with a browser-like User-Agent, `Accept-Language: en-US`,
/// and a 15-second timeout per request.
pub(crate) fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            USER_AGENT,
            reqwest::header::HeaderValue::from_static(BROWSER_UA),
        );
        headers.insert(
            ACCEPT_LANGUAGE,
            reqwest::header::HeaderValue::from_static("en-US,en;q=0.9"),
        );
        reqwest::Client::builder()
            .default_headers(headers)
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .expect("failed to build shared HTTP client")
    })
}
