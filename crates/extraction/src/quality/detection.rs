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

use regex::Regex;
use std::sync::OnceLock;

use super::types::{ContentQuality, QualityReason};

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
        static RE: OnceLock<Regex> = OnceLock::new();
        let re = RE.get_or_init(|| {
            Regex::new(
                "(?i)(checking your browser|cf-chl|turnstile|verify you are human|\
                 are you a human|browser integrity check|challenge-platform|\
                 datadome|just a moment\\.\\.\\.|checking the browser|cloudflare)",
            )
            .expect("valid regex")
        });
        re.is_match(text)
    }

    /// Detect CAPTCHA pages (reCAPTCHA, hCaptcha, Turnstile).
    ///
    /// Returns `true` if any CAPTCHA pattern is found in the text.
    pub fn detect_captcha(text: &str) -> bool {
        static RE: OnceLock<Regex> = OnceLock::new();
        let re = RE.get_or_init(|| {
            Regex::new("(?i)(captcha|recaptcha|g-recaptcha|h-captcha|turnstile|cf-turnstile)")
                .expect("valid regex")
        });
        re.is_match(text)
    }

    /// Detect paywall / subscription prompts.
    ///
    /// Returns `true` if any paywall pattern is found in the text.
    pub fn detect_paywall(text: &str) -> bool {
        static RE: OnceLock<Regex> = OnceLock::new();
        let re = RE.get_or_init(|| {
            Regex::new(
                "(?i)(subscribe( to)? (for|to read|to continue|now)|\
                 log in to read|log in to continue|sign in to read|\
                 sign in to continue|you've read your free articles|\
                 this is a subscriber|subscription required|\
                 support our (journalism|newsroom)|become a subscriber|\
                 already a subscriber|read the full article.*subscribe|\
                 continue reading.*subscribe|unlimited (access|digital) access|\
                 paid (article|content)|this article is (behind a|exclusively for))",
            )
            .expect("valid regex")
        });
        re.is_match(text)
    }

    /// Detect JS-required empty shells that render nothing useful.
    ///
    /// Returns `true` if the text appears to be an empty page shell that
    /// requires JavaScript to render actual content.
    pub fn detect_empty_shell(text: &str) -> bool {
        if text.len() < 80 {
            return true;
        }

        // JS-required message
        static JS_RE: OnceLock<Regex> = OnceLock::new();
        let js_re =
            JS_RE.get_or_init(|| Regex::new(r"(?i)please enable javascript").expect("valid regex"));
        if js_re.is_match(text) {
            return true;
        }

        // Fewer than 10 words — likely navigation chrome only
        if text.split_whitespace().count() < 10 {
            return true;
        }

        false
    }
}

/// Check if text contains sentence-ending punctuation.
pub(crate) fn regex_has_punctuation(text: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"[.!?]").expect("valid regex"));
    re.is_match(text)
}

/// Check if text contains words with 4+ characters.
pub(crate) fn regex_has_long_words(text: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"\w{4,}").expect("valid regex"));
    re.is_match(text)
}

/// Check if text contains paragraph breaks (double newline).
pub(crate) fn regex_has_paragraphs(text: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"\n\n").expect("valid regex"));
    re.is_match(text)
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
