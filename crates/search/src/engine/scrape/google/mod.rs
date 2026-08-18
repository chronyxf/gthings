//! Google search backend via CDP.
//!
//! Mirrors the original Google search flow in [`crate::search`]: navigates
//! to the SERP with `q`/`num`/`hl` params, detects CAPTCHA/access-denied
//! pages, scrolls to trigger lazy loading, extracts organic results with
//! the shared `search_extract.js` template, and post-processes them
//! (junk filter, `#:~:text=` strip, empty-snippet drop, base-URL dedup,
//! title/snippet cleaning, 1-based position renumbering).
//!
//! The backend owns its tab: each search creates a background tab, and the
//! tab is *always* closed afterwards (close failures are logged, not
//! propagated). Unlike the legacy `crate::search` implementation,
//! provenance and domain-authority construction are omitted — the
//! orchestrator computes those later.
//!
//! The navigation/CAPTCHA/extract/scroll pipeline is shared with the other
//! CDP backends in [`crate::engine::scrape::brave::shared`] (parameterized
//! by [`CdpSearchSpec`]); CAPTCHA detection lives in [`post`].

use std::sync::Arc;

use gthings_cdp::Session;

use crate::engine::scrape::brave::shared::{self, CdpSearchSpec};
use crate::engine::{
    EngineSearchResult, SearchEngine, SearchEngineBackend, SearchEngineError, SearchOptions,
};

mod post;

use post::{is_captcha_title, is_captcha_url};

/// Google search backend driving a real browser via CDP.
pub struct GoogleBackend {
    session: Arc<Session>,
}

impl GoogleBackend {
    /// Create a backend bound to the given browser session.
    pub fn new(session: Arc<Session>) -> Self {
        Self { session }
    }

    /// Build the shared-flow spec for `query`: Google SERP URL plus the
    /// Google-specific CAPTCHA predicates, result selector, and extraction
    /// template.
    fn spec(query: &str, count: usize) -> CdpSearchSpec<'static> {
        let params: String = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("q", query)
            .append_pair("num", &(count * 2).clamp(10, 100).to_string())
            .append_pair("hl", "en")
            .finish();
        CdpSearchSpec {
            url: format!("https://www.google.com/search?{params}"),
            engine: SearchEngine::Google,
            page_label: "Google",
            block_desc: "access-denied page",
            result_selector: "div[data-hveid], div[data-sokoban-container]",
            is_captcha_url,
            is_captcha_title,
            template: shared::GOOGLE_TEMPLATE,
        }
    }
}

impl SearchEngineBackend for GoogleBackend {
    fn name(&self) -> SearchEngine {
        SearchEngine::Google
    }

    fn requires_browser(&self) -> bool {
        true
    }

    async fn search(
        &self,
        query: &str,
        count: usize,
        _options: &SearchOptions,
    ) -> Result<Vec<EngineSearchResult>, SearchEngineError> {
        let results =
            shared::search_with_retry(&self.session, query, count, |q| Self::spec(q, count))
                .await?;
        tracing::debug!("google: {query} -> {} results", results.len());
        Ok(results)
    }
}
