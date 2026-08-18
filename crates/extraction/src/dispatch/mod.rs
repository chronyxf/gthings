//! Auto-dispatch of extraction to the right extractor based on URL type.
//!
//! [`AutoExtractor`] is the crate's top-level entry point used by the CLI and
//! the serve daemon. Routing logic is split across focused submodules:
//!
//! - [`rate_limit`] — HTTP 429 detection and `Retry-After` parsing.
//! - [`arxiv`] — arXiv URL handling (PDF download + abstract-page merge).
//! - [`github`] — GitHub URL routing, raw-file fetch, README resolution.
//! - [`language`] — file-extension → programming-language detection.

pub mod arxiv;
pub mod github;
pub mod language;
pub mod rate_limit;

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use gthings_common::pagination::{ExtractParams, Pagination};
use gthings_common::provenance::Provenance;

use crate::article::{
    Article, ContentTree, ContinuationSignals, ExtractionError, ExtractionInfo, ExtractionMethod,
    QualityScore, SourceInfo,
};
use crate::extractor::{Extractor, SourceType};
use crate::pdf::PdfExtractor;
use crate::web::WebExtractor;

// Re-exported so callers can keep using `crate::dispatch::check_rate_limit`
// (e.g. `web/mod.rs`) alongside the direct `rate_limit` module path.
pub(crate) use self::rate_limit::check_rate_limit;

/// Fetch a URL and return its body bytes, applying rate-limit detection.
///
/// Shared by the PDF, GitHub, and web fetch paths so HTTP error handling and
/// rate-limit detection stay in one place. `label` names the fetch in error
/// messages (e.g. "pdf", "github raw", "web").
pub(crate) async fn fetch_bytes(
    client: &reqwest::Client,
    url: &str,
    label: &str,
) -> Result<Vec<u8>, ExtractionError> {
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| ExtractionError::Http(format!("{label} fetch: {e}")))?;
    let status = resp.status();
    if !status.is_success() {
        check_rate_limit(&resp, format!("Rate limited while fetching {url}"))?;
        return Err(ExtractionError::Http(format!("HTTP {status} for {label}")));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| ExtractionError::Http(format!("{label} read: {e}")))?;
    Ok(bytes.to_vec())
}

/// Apply offset and max_chars slicing to extracted text.
pub(crate) fn slice_text(text: &str, params: &ExtractParams) -> String {
    text.chars()
        .skip(params.offset)
        .take(params.max_chars)
        .collect()
}

/// Assemble an [`Article`] from its common pieces, centralizing the
/// [`ExtractionInfo`] construction shared by every extractor.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_article(
    url: String,
    title: String,
    source: SourceInfo,
    method: ExtractionMethod,
    confidence: f64,
    duration_ms: u64,
    body: ContentTree,
    signals: ContinuationSignals,
    quality: QualityScore,
    provenance: Provenance,
    pagination: Pagination,
) -> Article {
    let now = chrono::Utc::now();
    Article {
        url,
        title,
        source,
        extraction: ExtractionInfo {
            method,
            confidence,
            accessed_at: now.to_rfc3339(),
            duration_ms,
        },
        body,
        signals,
        quality,
        provenance: Some(provenance),
        pagination: Some(pagination),
    }
}

/// Auto-dispatches extraction to the right extractor based on URL.
pub struct AutoExtractor {
    client: reqwest::Client,
    web: WebExtractor,
    pdf: PdfExtractor,
    max_content_bytes: u64,
    /// Cached default-branch candidates per `owner/repo`, resolved lazily via
    /// the GitHub API so a repo-root extraction only pays one extra request
    /// per unique repository (never per extraction).
    default_branches: Mutex<HashMap<String, Vec<String>>>,
}

impl AutoExtractor {
    /// Create a new AutoExtractor with a shared HTTP client reference.
    /// The client is cloned internally; callers can pass `http_client()` directly.
    pub fn new(client: &reqwest::Client) -> Self {
        let client = client.clone();
        let web = WebExtractor::new(client.clone());
        Self {
            client,
            web,
            pdf: PdfExtractor,
            max_content_bytes: 50 * 1024 * 1024,
            default_branches: Mutex::new(HashMap::new()),
        }
    }

    /// Extract content from a URL, auto-detecting the source type.
    async fn dispatch_extract(
        &self,
        url: &str,
        params: ExtractParams,
    ) -> Result<Article, ExtractionError> {
        let source_type = SourceType::from_url(url);

        match source_type {
            SourceType::Arxiv => self.extract_arxiv(url, params).await,
            SourceType::Pdf => self.extract_pdf(url, params).await,
            SourceType::GitHub => self.extract_github(url, params).await,
            SourceType::Web => self.web.extract(url.to_string(), params).await,
        }
    }

    /// Handle PDF URLs: download bytes, then extract.
    async fn extract_pdf(
        &self,
        url: &str,
        params: ExtractParams,
    ) -> Result<Article, ExtractionError> {
        let bytes = fetch_bytes(&self.client, url, "pdf").await?;

        if bytes.len() as u64 > self.max_content_bytes {
            return Err(ExtractionError::Http(format!(
                "PDF too large: {} bytes (max {})",
                bytes.len(),
                self.max_content_bytes
            )));
        }

        self.pdf.extract((url.to_string(), bytes), params).await
    }
}

#[async_trait]
impl Extractor for AutoExtractor {
    type Input = String;

    async fn extract(
        &self,
        input: String,
        params: ExtractParams,
    ) -> Result<Article, ExtractionError> {
        self.dispatch_extract(&input, params).await
    }
}
