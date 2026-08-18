/// PDF metadata extraction and PdfExtractor API.
///
/// Extracts text via `pdf-extract` (bundled MuPDF, no system dependencies).
mod metadata;

use async_trait::async_trait;
use gthings_common::pagination::ExtractParams;
use gthings_common::provenance::{ExtractionMethod as ProvenanceMethod, Provenance};
use pdf_extract::extract_text_from_mem;
use regex::bytes::Regex;
use std::sync::LazyLock;
use std::time::Instant;

use crate::article::{
    Article, ContentTree, ContinuationSignals, ExtractionError, ExtractionInfo, ExtractionMethod,
    QualityScore, SourceInfo,
};
use crate::extractor::Extractor;

use metadata::{extract_pdf_info_metadata, extract_pdf_xmp_metadata, normalize_pdf_date};

/// PDF text extractor using `pdf-extract` (bundled MuPDF).
///
/// # Examples
///
/// ```
/// use gthings_common::pagination::ExtractParams;
/// use gthings_extraction::PdfExtractor;
///
/// // Build a minimal valid PDF in memory (no file or browser needed).
/// let stream = "BT /F1 24 Tf 100 700 Td (Hello PDF) Tj ET\n";
/// let objects = [
///     "<< /Type /Catalog /Pages 2 0 R >>",
///     "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
///     "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
///     &format!("<< /Length {} >>\nstream\n{}endstream", stream.len(), stream),
///     "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
/// ];
/// let mut pdf = String::from("%PDF-1.4\n");
/// let mut offsets = Vec::new();
/// for (i, body) in objects.iter().enumerate() {
///     offsets.push(pdf.len());
///     pdf.push_str(&format!("{} 0 obj\n{}\nendobj\n", i + 1, body));
/// }
/// let xref_pos = pdf.len();
/// pdf.push_str(&format!("xref\n0 {}\n", objects.len() + 1));
/// pdf.push_str("0000000000 65535 f \n");
/// for off in &offsets {
///     pdf.push_str(&format!("{:010} 00000 n \n", off));
/// }
/// pdf.push_str(&format!(
///     "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
///     objects.len() + 1,
///     xref_pos
/// ));
///
/// let extractor = PdfExtractor;
/// let article = extractor
///     .extract_article("https://example.com/doc.pdf", pdf.as_bytes(), &ExtractParams::default())
///     .unwrap();
/// ```
pub struct PdfExtractor;

impl PdfExtractor {
    /// Extract PDF content as an Article.
    ///
    /// Uses `pdf-extract` (bundled MuPDF) to extract text from PDF bytes.
    /// `params` controls offset/max_chars slicing of the extracted text.
    pub fn extract_article(
        &self,
        url: &str,
        bytes: &[u8],
        params: &ExtractParams,
    ) -> Result<Article, ExtractionError> {
        let start = Instant::now();

        if !Self::is_pdf(bytes) {
            return Err(ExtractionError::Parse("not a valid PDF".into()));
        }

        let text = match Self::try_pdf_extract(bytes) {
            Some(t) => t,
            None => {
                return Err(ExtractionError::Empty(
                    "pdf-extract could not extract any text from PDF".into(),
                ));
            }
        };

        let pages = Self::count_pages(bytes);
        let total_len = text.len();
        let duration_ms = start.elapsed().as_millis() as u64;

        // Apply offset and max_chars slicing
        let effective_text: String = text
            .chars()
            .skip(params.offset)
            .take(params.max_chars)
            .collect();
        let effective_len = effective_text.len();

        let pagination = gthings_common::pagination::build_pagination(params, total_len);

        // Extract PDF metadata — try /Info dict first, fall back to XMP
        let pdf_meta = extract_pdf_info_metadata(bytes)
            .ok()
            .flatten()
            .or_else(|| extract_pdf_xmp_metadata(bytes).ok().flatten());

        let author = pdf_meta.as_ref().and_then(|m| m.author.clone());
        let published = pdf_meta.as_ref().and_then(|m| {
            m.creation_date
                .as_ref()
                .and_then(|d| normalize_pdf_date(d))
                .or_else(|| m.creation_date.clone())
        });
        let title = pdf_meta.as_ref().and_then(|m| m.title.clone());
        let source_site = pdf_meta
            .as_ref()
            .and_then(|m| m.creator.clone())
            .unwrap_or_default();

        let validated = crate::ContentQuality::validate(&effective_text);
        let quality = QualityScore {
            score: validated.score,
            is_ok: validated.is_ok,
            reasons: validated
                .reasons
                .iter()
                .map(|r| r.as_str().to_string())
                .collect(),
            entropy_bits_per_char: validated.entropy_bits_per_char,
        };

        let now = chrono::Utc::now();
        let provenance = Provenance {
            source_url: url.to_string(),
            method: ProvenanceMethod::Pdf,
            agent: gthings_common::user_agent::gthings_agent(),
            accessed_at: now,
            duration_ms,
        };

        Ok(Article {
            url: url.to_string(),
            title: title.unwrap_or_default(),
            source: SourceInfo {
                author,
                published,
                site_name: source_site,
                domain_authority: crate::article::round_score(
                    crate::extractor::compute_domain_authority(url),
                ),
                language: None,
            },
            extraction: ExtractionInfo {
                method: ExtractionMethod::PdfText,
                confidence: quality.score,
                accessed_at: now.to_rfc3339(),
                duration_ms,
            },
            body: ContentTree::Pdf {
                pages,
                text: effective_text,
                has_toc: false,
            },
            signals: ContinuationSignals {
                truncated: pagination.truncated,
                total_length: total_len,
                returned_length: effective_len,
                is_paywall: false,
                is_bot_blocked: false,
                is_empty_shell: total_len < 200,
                related_urls: Vec::new(),
            },
            quality,
            provenance: Some(provenance),
            pagination: Some(pagination),
        })
    }

    /// Check if bytes start with the PDF magic number `%PDF-`.
    fn is_pdf(bytes: &[u8]) -> bool {
        bytes.len() >= 5 && bytes[..5] == *b"%PDF-"
    }

    /// Count pages in PDF by counting `/Type /Page` entries (heuristic).
    ///
    /// The `[^s]` guard excludes `/Type /Pages` (the page-tree node) so only
    /// leaf page objects are counted. Failure mode: this is a raw byte-count
    /// heuristic, not a structural parse — it can over-count when `/Type /Page`
    /// appears inside compressed object streams (which are not decompressed
    /// here) or in comments/strings, and under-count when page objects are
    /// written with unusual whitespace. It is only used for the `pages` field
    /// in `ContentTree::Pdf` and never gates extraction.
    fn count_pages(bytes: &[u8]) -> usize {
        static PAGE_RE: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"/Type\s*/Page[^s]")
                .expect("valid static regex: /Type /Page pattern in PDF content")
        });
        PAGE_RE.find_iter(bytes).count()
    }

    /// PDF text extraction via `pdf-extract` (bundled MuPDF).
    ///
    /// Parses the PDF in memory and extracts text from all pages.
    /// Returns `None` when the document cannot be parsed or yields no text.
    fn try_pdf_extract(bytes: &[u8]) -> Option<String> {
        match extract_text_from_mem(bytes) {
            Ok(text) => {
                let text = text.trim().to_string();
                if text.is_empty() { None } else { Some(text) }
            }
            Err(_) => None,
        }
    }
}

#[async_trait]
impl Extractor for PdfExtractor {
    type Input = (String, Vec<u8>);

    async fn extract(
        &self,
        input: (String, Vec<u8>),
        params: ExtractParams,
    ) -> Result<Article, ExtractionError> {
        let (url, bytes) = input;
        self.extract_article(&url, &bytes, &params)
    }
}
