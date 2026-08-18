//! arXiv URL handling: download the PDF for full extraction, then merge with
//! the abstract-page metadata.

use gthings_common::pagination::ExtractParams;

use crate::article::{Article, ExtractionError, ExtractionMethod};
use crate::extractor::Extractor;

use super::AutoExtractor;

impl AutoExtractor {
    /// Handle arXiv URLs: download the PDF for full extraction,
    /// then merge with abstract page metadata.
    pub(super) async fn extract_arxiv(
        &self,
        url: &str,
        params: ExtractParams,
    ) -> Result<Article, ExtractionError> {
        // Normalize URL: ensure we have a /pdf/ URL for downloading
        let pdf_url = if url.contains("/abs/") {
            url.replace("/abs/", "/pdf/")
        } else if !url.contains("/pdf/") {
            format!("{}/pdf", url.trim_end_matches('/'))
        } else {
            url.to_string()
        };

        // Also get the abstract URL for metadata
        let abs_url = pdf_url
            .replace("/pdf/", "/abs/")
            .trim_end_matches(".pdf")
            .to_string();

        // Try to download and extract the PDF
        let pdf_result = self.extract_pdf(&pdf_url, params).await;

        let mut article = match pdf_result {
            Ok(mut article) => {
                // Try to get richer metadata from abstract page
                if article.source.author.is_none() || article.source.published.is_none() {
                    if let Ok(abs_article) =
                        self.web.extract(abs_url, ExtractParams::default()).await
                    {
                        merge_metadata(&mut article, &abs_article);
                    }
                }

                article
            }
            Err(_) => {
                // Fall back to abstract page HTML extraction
                self.web.extract(abs_url, ExtractParams::default()).await?
            }
        };

        // Mark as arXiv extraction; the quality score is already rounded by
        // the underlying extractor, so re-rounding would be a no-op.
        article.extraction.method = ExtractionMethod::ArxivOai;
        article.extraction.confidence = article.quality.score;

        Ok(article)
    }
}

/// Merge richer abstract-page metadata into the PDF-extracted article,
/// filling only the fields the PDF extraction left empty (author, published,
/// site_name, title). The PDF body is authoritative; abstract-page metadata
/// only backfills gaps.
fn merge_metadata(article: &mut Article, abs_article: &Article) {
    if article.source.author.is_none() {
        article.source.author = abs_article.source.author.clone();
    }
    if article.source.published.is_none() {
        article.source.published = abs_article.source.published.clone();
    }
    if article.source.site_name.is_empty() {
        article.source.site_name = abs_article.source.site_name.clone();
    }
    if article.title.is_empty() {
        article.title = abs_article.title.clone();
    }
}
