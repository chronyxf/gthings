//! Multi-engine search abstraction.
//!
//! Defines the [`SearchEngine`] enumeration, the [`SearchEngineBackend`]
//! trait implemented by concrete engine backends, shared result/error types,
//! and crate-internal HTTP infrastructure used by the HTTP-based backends.
//! Concrete backends are split into [`api`](api) (vendor API / JSON) and
//! [`scrape`](scrape) (non-API / HTML extraction) groups.
//!
//! Concrete types live in [`types`](types), HTTP helpers in [`http`](http).

use std::future::Future;

pub mod api;
pub mod html;
mod http;
pub mod pacing;
pub mod router;
pub mod scrape;
pub mod technique;
mod types;

pub use api::brave::BraveApiBackend;
pub use api::tavily::TavilyBackend;
pub use scrape::bing::BingBackend;
pub use scrape::brave::BraveBackend;
pub use scrape::google::GoogleBackend;

pub(crate) use http::{http_client, send_and_map};
pub use types::{
    EngineChoice, EngineMode, EngineSearchResult, SearchEngine, SearchEngineError, SearchOptions,
};
pub(crate) use types::{MAX_CONCURRENT_TABS, OP_TIMEOUT, env_var_from};

/// Implemented by each concrete engine backend (scrape/brave, scrape/bing,
/// scrape/google, api/brave, api/tavily, ...).
// Trait used statically only (never dyn); futures are Send — see impls in
// scrape/brave, scrape/bing, scrape/google, api/brave, api/tavily.
pub trait SearchEngineBackend: Send + Sync {
    /// The engine this backend implements.
    fn name(&self) -> SearchEngine;

    /// Whether this backend needs a browser session (CDP) to search.
    fn requires_browser(&self) -> bool {
        self.name().requires_browser()
    }

    /// Run a search for `query`, returning up to `count` normalized results.
    /// `options` carries optional per-search tuning (freshness, search_depth)
    /// that backends may consume; backends that do not support them ignore it.
    fn search<'a>(
        &'a self,
        query: &'a str,
        count: usize,
        options: &'a SearchOptions,
    ) -> impl Future<Output = Result<Vec<EngineSearchResult>, SearchEngineError>> + Send + 'a;
}
