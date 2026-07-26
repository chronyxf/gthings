/// Quality flag for domain-level reputation tracking.
///
/// Re-exported from `gthings_common` to make it available under
/// `gthings_extraction::QualityFlag`.
pub use gthings_common::domain_reputation::QualityFlag;

/// Reason for a quality check failure.
///
/// Used in [`QualityResult::reasons`] to indicate why content failed
/// the quality gate. Each variant corresponds to a single static string
/// — no heap allocation required.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityReason {
    /// Content is completely empty.
    EmptyContent,
    /// Content is too short to be useful (< 80 chars).
    TooShort,
    /// Matched a browser error page pattern (e.g. "This site can't be reached").
    BrowserErrorPage,
    /// Matched a connection error pattern (e.g. ERR_CONNECTION).
    ConnectionError,
    /// Matched a 404 Not Found pattern.
    NotFound,
    /// Content is whitespace-only.
    WhitespaceOnly,
    /// Content is a paywall teaser (e.g. "Read More »").
    PaywallTeaser,
    /// Content is navigation chrome (short, no natural language).
    NavigationChrome,
    /// Content has too few words (< 15 words in short text).
    TooFewWords,
    /// Content has no punctuation (suggests machine output).
    NoPunctuation,
}

/// Result of a content quality validation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct QualityResult {
    /// Quality score from 0.0 to 1.0.
    pub score: f64,
    /// Whether the content passes the quality gate (score >= 0.5).
    pub is_ok: bool,
    /// List of reasons why content failed quality checks.
    pub reasons: Vec<QualityReason>,
    /// Length of the input text that was validated.
    pub length: usize,
    /// Character-level Shannon entropy (bits/char) of the extracted text.
    pub entropy_bits_per_char: f32,
    /// Domain-level quality flags derived from content analysis (e.g., ThinContent, Garbled).
    pub flags: Vec<QualityFlag>,
}

/// Content quality validation — all methods are stateless.
pub struct ContentQuality;
