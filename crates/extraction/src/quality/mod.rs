//! Content quality assessment: validation, detection, and entropy metrics.
//!
//! Split across submodules in the `quality/` directory:
//! - `mod.rs` — shared types, constants, and the entropy metric.
//! - `patterns.rs` — compiled regexes (bot/paywall/captcha detection, text
//!   metrics, boilerplate heuristics).
//! - `detect.rs` — page-signal detection ([`ContentQuality::detect_all`]) and the
//!   shared empty-shell heuristic.
//! - `validate.rs` — the [`ContentQuality::validate`] scoring pipeline and its tests.

mod detect;
mod patterns;
mod validate;

pub(crate) use detect::is_empty_shell;
pub use patterns::{NAV_TOKENS, is_nav_dense};

use std::collections::HashMap;

/// Shared constant for paywall teaser detection.
pub const READ_MORE_INDICATOR: &str = "Read More \u{00bb}";

/// Minimum content length (bytes) before text is considered substantive.
pub(crate) const MIN_CONTENT_LEN: usize = 80;
/// Minimum word count for short text to avoid the empty-shell heuristic.
pub(crate) const MIN_WORDS: usize = 15;

/// Penalty applied when content is too short to be useful.
pub(crate) const PENALTY_TOO_SHORT: f64 = 0.4;
/// Penalty applied for browser error / connection / 404 pages.
pub(crate) const PENALTY_ERROR_PAGE: f64 = 0.5;
/// Penalty applied for a paywall teaser.
pub(crate) const PENALTY_PAYWALL: f64 = 0.5;
/// Penalty applied for navigation chrome.
pub(crate) const PENALTY_NAV_CHROME: f64 = 0.3;
/// Penalty applied for boilerplate content.
pub(crate) const PENALTY_BOILERPLATE: f64 = 0.3;
/// Penalty applied for too few words.
pub(crate) const PENALTY_TOO_FEW_WORDS: f64 = 0.2;
/// Penalty applied for missing punctuation.
pub(crate) const PENALTY_NO_PUNCTUATION: f64 = 0.1;
/// Bonus for natural-language punctuation (smaller than the NoPunctuation penalty).
pub(crate) const BONUS_PUNCTUATION: f64 = 0.05;
/// Bonus for natural-language long words.
pub(crate) const BONUS_LONG_WORDS: f64 = 0.05;

/// Single source of truth mapping each quality reason to its score penalty.
pub(crate) const PENALTY_TABLE: &[(QualityReason, f64)] = &[
    (QualityReason::TooShort, PENALTY_TOO_SHORT),
    (QualityReason::BrowserErrorPage, PENALTY_ERROR_PAGE),
    (QualityReason::ConnectionError, PENALTY_ERROR_PAGE),
    (QualityReason::NotFound, PENALTY_ERROR_PAGE),
    (QualityReason::PaywallTeaser, PENALTY_PAYWALL),
    (QualityReason::NavigationChrome, PENALTY_NAV_CHROME),
    (QualityReason::Boilerplate, PENALTY_BOILERPLATE),
    (QualityReason::TooFewWords, PENALTY_TOO_FEW_WORDS),
    (QualityReason::NoPunctuation, PENALTY_NO_PUNCTUATION),
];

/// Look up the score penalty for a quality reason.
pub(crate) fn penalty_for(reason: QualityReason) -> f64 {
    PENALTY_TABLE
        .iter()
        .find(|(r, _)| *r == reason)
        .map(|(_, p)| *p)
        .unwrap_or(0.0)
}

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
    /// Content is boilerplate (image placeholders, share/listen prompts,
    /// category views, "featured" headers, or nav-link-dense menus).
    Boilerplate,
}

/// Stable snake_case string encoding, matching the `serde`
/// `rename_all = "snake_case"` serialization. Single source shared by
/// [`QualityReason::as_str`] and the wire format so the two can never drift.
const REASON_STR: &[(QualityReason, &str)] = &[
    (QualityReason::EmptyContent, "empty_content"),
    (QualityReason::TooShort, "too_short"),
    (QualityReason::BrowserErrorPage, "browser_error_page"),
    (QualityReason::ConnectionError, "connection_error"),
    (QualityReason::NotFound, "not_found"),
    (QualityReason::WhitespaceOnly, "whitespace_only"),
    (QualityReason::PaywallTeaser, "paywall_teaser"),
    (QualityReason::NavigationChrome, "navigation_chrome"),
    (QualityReason::TooFewWords, "too_few_words"),
    (QualityReason::NoPunctuation, "no_punctuation"),
    (QualityReason::Boilerplate, "boilerplate"),
];

impl QualityReason {
    /// Stable snake_case string encoding, matching the `serde`
    /// `rename_all = "snake_case"` serialization. Used wherever reasons are
    /// surfaced as strings (e.g. `QualityScore::reasons`) instead of relying
    /// on `Debug` formatting.
    pub(crate) fn as_str(&self) -> &'static str {
        REASON_STR
            .iter()
            .find(|(r, _)| *r == *self)
            .map(|(_, s)| *s)
            .unwrap_or("unknown")
    }
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
}

impl crate::article::QualityScore {
    /// Build a [`crate::article::QualityScore`] from a typed [`QualityResult`],
    /// encoding reasons as stable snake_case strings.
    pub fn from_result(result: &QualityResult) -> Self {
        Self {
            score: result.score,
            is_ok: result.is_ok,
            reasons: result
                .reasons
                .iter()
                .map(|r| r.as_str().to_string())
                .collect(),
            entropy_bits_per_char: result.entropy_bits_per_char,
        }
    }
}

/// Content quality validation — all methods are stateless.
pub struct ContentQuality;

/// Compute the character-level Shannon entropy of a string in bits per character.
///
/// H = - Σ p(c) · log₂(p(c))
///
/// where p(c) is the relative frequency of each Unicode character in `text`.
///
/// Returns 0.0 for empty or whitespace-only strings.
pub fn shannon_entropy(text: &str) -> f32 {
    let text = text.trim();
    if text.is_empty() {
        return 0.0;
    }

    // Single pass: count character frequencies
    let mut freq: HashMap<char, usize> = HashMap::new();
    for c in text.chars() {
        *freq.entry(c).or_insert(0) += 1;
    }

    // Total is the number of Unicode characters, not bytes, so non-ASCII text
    // computes entropy correctly.
    let total = text.chars().count() as f32;
    let mut entropy = 0.0_f32;

    for &count in freq.values() {
        if count == 0 {
            continue;
        }
        let p = count as f32 / total;
        entropy -= p * p.log2();
    }

    entropy
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_entropy_empty() {
        assert_eq!(shannon_entropy(""), 0.0);
        assert_eq!(shannon_entropy("   "), 0.0);
    }

    #[test]
    fn test_entropy_repeated_char() {
        let h = shannon_entropy("aaaaaaaaaa");
        assert!(
            h < 0.01,
            "entropy of repeated char should be near 0, got {h}"
        );
    }

    #[test]
    fn test_entropy_two_chars_equal() {
        // 5 'a's + 5 'b's → p = 0.5 each → H = 1.0 bit/char
        let h = shannon_entropy("aaaaabbbbb");
        let diff = (h - 1.0).abs();
        assert!(
            diff < 0.01,
            "entropy of 2-symbol uniform should be 1.0, got {h}"
        );
    }

    #[test]
    fn test_entropy_uniform_alphabet() {
        // 4 distinct chars, equal freq → H = log2(4) = 2.0
        let s: String = (0..100).map(|i| char::from(b'a' + (i % 4))).collect();
        let h = shannon_entropy(&s);
        let diff = (h - 2.0).abs();
        assert!(
            diff < 0.05,
            "entropy of 4-symbol uniform should be ~2.0, got {h}"
        );
    }

    #[test]
    fn test_entropy_english_paragraph() {
        let paragraph = "The quick brown fox jumps over the lazy dog. This classic pangram contains every letter of the English alphabet at least once. It has been used for typing practice and font display for decades.";
        let h = shannon_entropy(paragraph);
        assert!(
            (3.0..=5.5).contains(&h),
            "English paragraph entropy should be in 3.0-5.5 range, got {h}"
        );
    }

    #[test]
    fn test_entropy_single_char() {
        let h = shannon_entropy("x");
        assert!(h < 0.01, "single char entropy should be 0, got {h}");
    }

    #[test]
    fn test_entropy_non_ascii() {
        // Non-ASCII text: entropy is computed per character, not per byte.
        let text = "héllo wörld 日本語テスト";
        let h = shannon_entropy(text);
        assert!(
            h > 0.0,
            "non-ASCII text should have positive entropy, got {h}"
        );

        // A repeated single multi-byte char must have ~0 entropy regardless of
        // its byte width (é is 2 bytes in UTF-8).
        let repeated = "é".repeat(50);
        let h2 = shannon_entropy(&repeated);
        assert!(
            h2 < 0.01,
            "repeated non-ASCII char should have ~0 entropy, got {h2}"
        );
    }

    #[test]
    fn test_entropy_high_entropy() {
        // Many distinct chars, roughly uniform → high entropy
        let s: String = (0..=255).map(|i| char::from(b' ' + (i % 95))).collect();
        let h = shannon_entropy(&s);
        // max entropy for 95 printable ASCII chars ≈ log2(95) ≈ 6.57
        assert!(
            h > 5.5,
            "highly varied text should have entropy > 5.5, got {h}"
        );
    }
}
