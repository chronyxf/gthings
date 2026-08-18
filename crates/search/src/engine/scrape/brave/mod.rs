//! Brave search backend via CDP.
//!
//! Navigates the real browser to the SERP with `q`/`source`/`hl` params,
//! detects CAPTCHA/verification pages, scrolls to trigger lazy loading,
//! extracts organic results with the shared `brave_extract.js` template, and
//! post-processes them (junk filter, `#:~:text=` strip, empty-snippet drop,
//! base-URL dedup, title/snippet cleaning, 1-based position renumbering).
//! Plain-HTTP scraping is gone: Brave rate-limits HTTP scrapers aggressively
//! (429, ~60s pacing), so a rendered browser page via CDP is required.
//!
//! The backend owns its tab: each search creates a background tab, and the
//! tab is *always* closed afterwards (close failures are logged, not
//! propagated). The navigation/CAPTCHA/extract/scroll pipeline is shared
//! with the other CDP backends in [`shared`] (parameterized by
//! [`CdpSearchSpec`]); CAPTCHA detection lives in [`post`].

use std::sync::Arc;

use gthings_cdp::Session;

use crate::engine::{
    EngineSearchResult, SearchEngine, SearchEngineBackend, SearchEngineError, SearchOptions,
};

mod post;
pub(crate) mod shared;

use post::{is_captcha_title, is_captcha_url};
use shared::CdpSearchSpec;

/// Brave search backend driving a real browser via CDP.
pub struct BraveBackend {
    session: Arc<Session>,
}

impl BraveBackend {
    /// Create a backend bound to the given browser session.
    pub fn new(session: Arc<Session>) -> Self {
        Self { session }
    }

    /// Build the shared-flow spec for `query`: Brave SERP URL plus the
    /// Brave-specific CAPTCHA predicates, result selector, and extraction
    /// template.
    fn spec(query: &str) -> CdpSearchSpec<'static> {
        let params: String = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("q", query)
            .append_pair("source", "web")
            .append_pair("hl", "en")
            .finish();
        CdpSearchSpec {
            url: format!("https://search.brave.com/search?{params}"),
            engine: SearchEngine::Brave,
            page_label: "Brave",
            block_desc: "block page",
            result_selector: "div[data-type=\"web\"]",
            is_captcha_url,
            is_captcha_title,
            template: shared::BRAVE_TEMPLATE,
        }
    }
}

impl SearchEngineBackend for BraveBackend {
    fn name(&self) -> SearchEngine {
        SearchEngine::Brave
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
        let results = shared::search_with_retry(&self.session, query, count, Self::spec).await?;
        tracing::debug!("brave: {query} -> {} results", results.len());
        Ok(results)
    }
}
