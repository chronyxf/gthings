//! HTML web page extraction via a single-pass `scraper`-based DOM parse.
//!
//! [`WebExtractor`] fetches a URL, parses the HTML once (html5ever), and
//! extracts metadata, body text, headings, JSON‑LD, sections, and a quality
//! score from the resulting DOM.
//!
//! The module is split into focused submodules:
//! - `dom` — DOM helpers (`collect_text_nodes`) and `extract_from_html`.
//! - `sections` — heading/section tree building logic.
//! - `quality` — quality scoring wrapping `ContentQuality::validate`.
//!
//! [`WebExtractor`] itself and its [`Extractor`] impl live in this file.

mod dom;
mod quality;
mod sections;

use std::time::Instant;

use async_trait::async_trait;
use gthings_common::pagination::ExtractParams;
use gthings_common::provenance::{ExtractionMethod as ProvenanceMethod, Provenance};

use crate::article::{
    Article, ContentTree, ContinuationSignals, ExtractionError, ExtractionMethod,
};
use crate::extractor::Extractor;

/// Minimum HTML length (bytes) before a response is worth parsing.
const MIN_HTML_LEN: usize = 100;

/// Extracts content from HTML web pages using a single-pass `scraper`-based
/// DOM parse (html5ever), eliminating the double‑parse overhead of the
/// previous `scraper` + `readability` pipeline.
pub struct WebExtractor {
    client: reqwest::Client,
}

impl WebExtractor {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }
}

#[async_trait]
impl Extractor for WebExtractor {
    type Input = String;

    async fn extract(
        &self,
        url: String,
        params: ExtractParams,
    ) -> Result<Article, ExtractionError> {
        let start = Instant::now();

        let html_bytes = crate::dispatch::fetch_bytes(&self.client, &url, "web").await?;
        let html = String::from_utf8_lossy(&html_bytes).into_owned();

        if html.len() < MIN_HTML_LEN {
            return Err(ExtractionError::Empty("response too short".into()));
        }

        // Single‑pass extraction: metadata, title, full text, and headings
        // from one DOM traversal.
        let (source, title, full_text, headings) = Self::extract_from_html(&html, &url);

        if full_text.trim().is_empty() {
            return Err(ExtractionError::Empty(
                "extraction produced no content".into(),
            ));
        }

        let total_len = full_text.len();

        // Apply offset and max_chars slicing
        let effective_text = crate::dispatch::slice_text(&full_text, &params);

        let effective_len = effective_text.len();

        let pagination = gthings_common::pagination::build_pagination(&params, total_len);

        // Build sections from the streaming‑extracted headings
        let sections = Self::build_sections_from_headings(&headings, &effective_text);

        let quality_result = crate::ContentQuality::validate(&effective_text);
        let quality = Self::score_quality(&quality_result, &sections);

        let duration_ms = start.elapsed().as_millis() as u64;

        let signals = ContinuationSignals {
            truncated: pagination.truncated,
            total_length: total_len,
            returned_length: effective_len,
            is_paywall: quality_result
                .reasons
                .contains(&crate::quality::QualityReason::PaywallTeaser),
            is_bot_blocked: false,
            is_empty_shell: crate::quality::is_empty_shell(&full_text),
            related_urls: Vec::new(),
        };

        let provenance = Provenance::new(url.clone(), ProvenanceMethod::Follow, duration_ms);

        Ok(crate::dispatch::build_article(
            url,
            title,
            source,
            ExtractionMethod::Readability,
            quality.score,
            duration_ms,
            ContentTree::Article {
                sections,
                full_text: effective_text,
                total_length: total_len,
            },
            signals,
            quality,
            provenance,
            pagination,
        ))
    }
}
