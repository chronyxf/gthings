//! Server-side quality detection on extracted text.
//!
//! # In-browser pre-check (keep in sync)
//!
//! An equivalent (but lighter) JS snippet runs in the browser immediately
//! after navigation via [`Session::check_page_signals`] (in `cdp/src/session.rs`).
//! That snippet detects BotWall, Captcha, and Paywall using DOM queries
//! before the full extraction JS runs.
//!
//! The pattern lists below should be kept in sync with the JS snippet
//! (replicated in `cdp/src/session.rs`).
//!
//! When adding new patterns here, add corresponding DOM / text selectors
//! to the JS snippet so the early pre-check catches them too.

use regex::{Regex, RegexSet};
use std::sync::{LazyLock, OnceLock};

use super::types::{ContentQuality, QualityFlag, QualityReason};

// ---------------------------------------------------------------------------
// Shared pattern indices
// ---------------------------------------------------------------------------

const IDX_BOT: usize = 0;
const IDX_PAYWALL: usize = 1;
const IDX_CAPTCHA: usize = 2;
const IDX_EMPTY_SHELL: usize = 3;

/// All detection patterns compiled once into a single RegexSet.
static ALL_PATTERNS: LazyLock<RegexSet> = LazyLock::new(|| {
    RegexSet::new([
        // 0 — Bot challenge patterns
        r"(?i)(checking your browser|cf-chl|turnstile|verify you are human|are you a human|browser integrity check|challenge-platform|datadome|just a moment\.\.\.|checking the browser|cloudflare)",
        // 1 — Paywall / subscription prompt patterns
        r"(?i)(subscribe(?: to)? (?:for|to read|to continue|now)|log in to (?:read|continue)|sign in to (?:read|continue)|you've read your free articles|this is a subscriber|subscription required|support our (?:journalism|newsroom)|become a subscriber|already a subscriber|read the full article.*subscribe|continue reading.*subscribe|unlimited (?:access|digital) access|paid (?:article|content)|this article is (?:behind a|exclusively for))",
        // 2 — CAPTCHA patterns
        r"(?i)(captcha|recaptcha|g-recaptcha|h-captcha|turnstile|cf-turnstile)",
        // 3 — JavaScript-required empty shell
        r"(?i)please enable javascript",
    ])
    .expect("valid RegexSet for ALL_PATTERNS")
});

// ---------------------------------------------------------------------------
// Text-metric regexes (punctuation, long words, paragraphs)
// ---------------------------------------------------------------------------

const IDX_PUNCTUATION: usize = 0;
const IDX_LONG_WORDS: usize = 1;
const IDX_PARAGRAPHS: usize = 2;

static TEXT_METRICS: LazyLock<[Regex; 3]> = LazyLock::new(|| {
    [
        Regex::new(r"[.!?]").expect("valid regex: punctuation"),
        Regex::new(r"\w{4,}").expect("valid regex: long words"),
        Regex::new(r"\n\n").expect("valid regex: paragraphs"),
    ]
});

impl ContentQuality {
    /// Error page patterns (shared with detect methods).
    pub(crate) fn error_page_reasons() -> &'static [(Regex, QualityReason)] {
        static PATTERNS: OnceLock<Vec<(Regex, QualityReason)>> = OnceLock::new();
        PATTERNS.get_or_init(|| {
            vec![
                (
                    Regex::new(r"(?i)This site can't be reached").expect("valid regex"),
                    QualityReason::BrowserErrorPage,
                ),
                (
                    Regex::new(r"ERR_CONNECTION").expect("valid regex"),
                    QualityReason::ConnectionError,
                ),
                (
                    Regex::new(r"404 Not Found").expect("valid regex"),
                    QualityReason::NotFound,
                ),
                (
                    Regex::new(r"^\s*$").expect("valid regex"),
                    QualityReason::WhitespaceOnly,
                ),
            ]
        })
    }

    /// Detect all quality flags in a single pass over the text.
    ///
    /// Combines bot, captcha, paywall, and empty-shell detection into one
    /// regex traversal, then applies length / word-count heuristics.
    /// Returns the same flags as calling the four `detect_*` methods
    /// individually, but with fewer passes over the content.
    pub fn detect_all(text: &str) -> Vec<QualityFlag> {
        let matched = ALL_PATTERNS.matches(text);
        let mut flags = Vec::with_capacity(4);

        if matched.matched(IDX_BOT) {
            flags.push(QualityFlag::BotWall);
        }
        if matched.matched(IDX_PAYWALL) {
            flags.push(QualityFlag::Paywall);
        }
        if matched.matched(IDX_CAPTCHA) {
            flags.push(QualityFlag::Captcha);
        }

        // Empty shell: JS pattern, short length, or too few words
        let empty_shell = matched.matched(IDX_EMPTY_SHELL)
            || text.len() < 80
            || text.split_whitespace().count() < 10;
        if empty_shell {
            flags.push(QualityFlag::EmptyShell);
        }

        flags
    }

    /// Detect bot challenge pages (Cloudflare, DataDome, Turnstile, etc.).
    ///
    /// Returns `true` if any bot challenge pattern is found in the text.
    ///
    /// # Examples
    ///
    /// ```
    /// # use gthings_extraction::ContentQuality;
    /// assert!(ContentQuality::detect_bot("Checking your browser before accessing"));
    /// assert!(!ContentQuality::detect_bot("Normal article content here"));
    /// ```
    pub fn detect_bot(text: &str) -> bool {
        ALL_PATTERNS.matches(text).matched(IDX_BOT)
    }

    /// Detect CAPTCHA pages (reCAPTCHA, hCaptcha, Turnstile).
    ///
    /// Returns `true` if any CAPTCHA pattern is found in the text.
    pub fn detect_captcha(text: &str) -> bool {
        ALL_PATTERNS.matches(text).matched(IDX_CAPTCHA)
    }

    /// Detect paywall / subscription prompts.
    ///
    /// Returns `true` if any paywall pattern is found in the text.
    pub fn detect_paywall(text: &str) -> bool {
        ALL_PATTERNS.matches(text).matched(IDX_PAYWALL)
    }

    /// Detect JS-required empty shells that render nothing useful.
    ///
    /// Returns `true` if the text appears to be an empty page shell that
    /// requires JavaScript to render actual content.
    pub fn detect_empty_shell(text: &str) -> bool {
        if text.len() < 80 {
            return true;
        }
        if ALL_PATTERNS.matches(text).matched(IDX_EMPTY_SHELL) {
            return true;
        }
        // Fewer than 10 words — likely navigation chrome only
        text.split_whitespace().count() < 10
    }
}

/// Check if text contains sentence-ending punctuation.
pub(crate) fn regex_has_punctuation(text: &str) -> bool {
    TEXT_METRICS[IDX_PUNCTUATION].is_match(text)
}

/// Check if text contains words with 4+ characters.
pub(crate) fn regex_has_long_words(text: &str) -> bool {
    TEXT_METRICS[IDX_LONG_WORDS].is_match(text)
}

/// Check if text contains paragraph breaks (double newline).
pub(crate) fn regex_has_paragraphs(text: &str) -> bool {
    TEXT_METRICS[IDX_PARAGRAPHS].is_match(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_bot() {
        assert!(ContentQuality::detect_bot(
            "Checking your browser before accessing"
        ));
        assert!(ContentQuality::detect_bot("cloudflare challenge"));
        assert!(ContentQuality::detect_bot("Just a moment..."));
        assert!(!ContentQuality::detect_bot("Normal article text"));
    }

    #[test]
    fn test_detect_captcha() {
        assert!(ContentQuality::detect_captcha("recaptcha widget"));
        assert!(ContentQuality::detect_captcha("h-captcha"));
        assert!(ContentQuality::detect_captcha("cf-turnstile"));
        assert!(!ContentQuality::detect_captcha("normal content"));
    }

    #[test]
    fn test_detect_paywall() {
        assert!(ContentQuality::detect_paywall(
            "Subscribe now to continue reading"
        ));
        assert!(ContentQuality::detect_paywall(
            "Log in to read this article"
        ));
        assert!(ContentQuality::detect_paywall(
            "You've read your free articles"
        ));
        assert!(!ContentQuality::detect_paywall("This is a normal article"));
    }

    #[test]
    fn test_detect_empty_shell() {
        assert!(ContentQuality::detect_empty_shell("short"));
        assert!(ContentQuality::detect_empty_shell(
            "Please enable JavaScript to view this page."
        ));
        assert!(!ContentQuality::detect_empty_shell(
            "This is a sufficiently long text with many words that should not be detected as an empty shell."
        ));
    }
}
