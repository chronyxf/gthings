//! Page-signal detection: bot, CAPTCHA, paywall, and empty-shell detection.

use regex::Regex;
use std::sync::OnceLock;

use super::patterns::{
    ALL_PATTERNS, IDX_BOT, IDX_CAPTCHA, IDX_EMPTY_SHELL, IDX_PAYWALL, is_nav_dense,
    regex_has_punctuation,
};
use super::{ContentQuality, MIN_CONTENT_LEN, MIN_WORDS, QualityFlag, QualityReason};

/// Determine whether text is an empty shell.
///
/// - < [`MIN_CONTENT_LEN`] chars: always a shell (overlaps with TooShort).
/// - JS-required pattern: always a shell.
/// - 80-120 char band: only a shell when an additional signal is present
///   (no punctuation, or nav-dense tokens) so short-but-valid prose is not
///   penalized.
/// - >= 120 chars: shell only when it has very few words.
pub(crate) fn is_empty_shell(text: &str) -> bool {
    let len = text.len();
    if len < MIN_CONTENT_LEN {
        return true;
    }
    if ALL_PATTERNS.matches(text).matched(IDX_EMPTY_SHELL) {
        return true;
    }
    if len < 120 {
        return !regex_has_punctuation(text) || is_nav_dense(text);
    }
    text.split_whitespace().count() < MIN_WORDS
}

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
        if is_empty_shell(text) {
            flags.push(QualityFlag::EmptyShell);
        }

        flags
    }
}
