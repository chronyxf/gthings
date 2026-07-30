use gthings_common::pagination::Pagination;
use gthings_common::provenance::Provenance;
use serde::{Deserialize, Serialize};

/// The root content type returned by every extractor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Article {
    pub url: String,
    pub title: String,
    pub source: SourceInfo,
    pub extraction: ExtractionInfo,
    pub body: ContentTree,
    pub signals: ContinuationSignals,
    pub quality: QualityScore,
    /// Provenance chain: how this content was discovered/acquired.
    pub provenance: Option<Provenance>,
    /// Pagination state: offset, truncation, continuation token.
    pub pagination: Option<Pagination>,
}

/// Provenance metadata about the source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceInfo {
    pub author: Option<String>,
    pub published: Option<String>, // ISO 8601 date string
    pub site_name: String,
    pub domain_authority: f64, // 0.0-1.0
    pub language: Option<String>,
}

/// How this content was extracted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractionInfo {
    pub method: ExtractionMethod,
    pub confidence: f64,     // 0.0-1.0
    pub accessed_at: String, // ISO 8601
    pub duration_ms: u64,
}

/// The extraction method used.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ExtractionMethod {
    Readability,
    Cetd,
    PdfText,
    RawFile,
    ArxivOai,
    CdpEvaluate,
}

/// The extracted content body — can be structured article, code, or PDF.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ContentTree {
    Article {
        sections: Vec<Section>,
        full_text: String,
        total_length: usize,
    },
    Code {
        language: String,
        content: String,
        file_path: String,
        line_count: usize,
    },
    Pdf {
        pages: usize,
        text: String,
        has_toc: bool,
    },
}

/// A single section/heading within an article.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Section {
    pub heading: String,
    pub depth: u8,       // 1=h1, 2=h2, etc.
    pub offset: usize,   // char offset in full_text
    pub length: usize,   // char length
    pub content: String, // section text
    pub subsections: Vec<Section>,
}

/// AI-agent hints for continuation and quality assessment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContinuationSignals {
    pub truncated: bool,
    pub total_length: usize,
    pub returned_length: usize,
    pub is_paywall: bool,
    pub is_bot_blocked: bool,
    pub is_empty_shell: bool,
    pub related_urls: Vec<String>,
}

/// Content quality assessment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityScore {
    pub score: f64,  // 0.0-1.0
    pub is_ok: bool, // score >= 0.5
    pub reasons: Vec<String>,
    /// Character-level Shannon entropy (bits/char) of extracted text.
    /// High entropy suggests varied/garbled text; low entropy suggests
    /// repetitive/thin content. Set to 0.0 when unavailable.
    #[serde(default)]
    pub entropy_bits_per_char: f32,
}

/// Errors that can occur during extraction.
#[derive(Debug, thiserror::Error)]
pub enum ExtractionError {
    #[error("HTTP error: {0}")]
    Http(String),
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("Unsupported: {0}")]
    Unsupported(String),
    #[error("Empty content: {0}")]
    Empty(String),
    #[error("Bot blocked: {0}")]
    BotBlocked(String),
    #[error("Timeout: {0}")]
    Timeout(String),
    #[error("Rate limited (HTTP 429): {detail}")]
    RateLimited {
        detail: String,
        retry_after: Option<u64>,
    },
}

/// Round a score to 2 decimal places to avoid floating-point artifacts in JSON output.
///
/// f64 arithmetic (e.g. `1.0 - 0.4 - 0.2`) can produce `0.39999999999999997` in JSON.
/// Rounding to 2dp eliminates these artifacts while preserving sufficient precision.
pub(crate) fn round_score(score: f64) -> f64 {
    (score * 100.0).round() / 100.0
}
