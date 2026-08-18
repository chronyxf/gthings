//! Compiled detection and text-metric regexes for content quality.
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
use std::sync::LazyLock;

/// Index of the bot-challenge pattern within [`ALL_PATTERNS`].
pub(super) const IDX_BOT: usize = 0;
/// Index of the paywall pattern within [`ALL_PATTERNS`].
pub(super) const IDX_PAYWALL: usize = 1;
/// Index of the CAPTCHA pattern within [`ALL_PATTERNS`].
pub(super) const IDX_CAPTCHA: usize = 2;
/// Index of the empty-shell pattern within [`ALL_PATTERNS`].
pub(super) const IDX_EMPTY_SHELL: usize = 3;

/// All detection patterns compiled once into a single RegexSet.
pub(super) static ALL_PATTERNS: LazyLock<RegexSet> = LazyLock::new(|| {
    RegexSet::new([
        // 0 — Bot challenge patterns. Note: `turnstile` is deliberately NOT
        // here — it is Cloudflare's CAPTCHA product, so it belongs to the
        // CAPTCHA pattern (index 2) only, avoiding a double-match.
        r"(?i)(checking your browser|cf-chl|verify you are human|are you a human|browser integrity check|challenge-platform|datadome|just a moment\.\.\.|checking the browser|cloudflare)",
        // 1 — Paywall / subscription prompt patterns
        r"(?i)(subscribe(?: to)? (?:for|to read|to continue|now)|log in to (?:read|continue)|sign in to (?:read|continue)|you've read your free articles|this is a subscriber|subscription required|support our (?:journalism|newsroom)|become a subscriber|already a subscriber|read the full article.*subscribe|continue reading.*subscribe|unlimited (?:access|digital) access|paid (?:article|content)|this article is (?:behind a|exclusively for))",
        // 2 — CAPTCHA patterns (turnstile is Cloudflare's CAPTCHA product)
        r"(?i)(captcha|recaptcha|g-recaptcha|h-captcha|turnstile|cf-turnstile)",
        // 3 — JavaScript-required empty shell
        r"(?i)(please enable javascript|try using)",
    ])
    .expect("valid RegexSet for ALL_PATTERNS")
});

/// Punctuation and long-word regexes used by the scoring pipeline.
pub(super) struct TextMetrics {
    pub(super) punctuation: Regex,
    pub(super) long_words: Regex,
}

/// Punctuation and long-word regexes compiled once, keyed by name instead of
/// a positional array index.
pub(super) static TEXT_METRICS: LazyLock<TextMetrics> = LazyLock::new(|| TextMetrics {
    punctuation: Regex::new(r"[.!?]").expect("valid regex: punctuation"),
    long_words: Regex::new(r"\w{4,}").expect("valid regex: long words"),
});

/// Check if text contains sentence-ending punctuation.
pub(super) fn regex_has_punctuation(text: &str) -> bool {
    TEXT_METRICS.punctuation.is_match(text)
}

/// Check if text contains words with 4+ characters.
pub(super) fn regex_has_long_words(text: &str) -> bool {
    TEXT_METRICS.long_words.is_match(text)
}

/// Unambiguous nav-menu tokens shared by the nav-dense checks. Common tech
/// words (cloud, services, support, managed, solutions, partners, consulting)
/// are deliberately excluded because they appear frequently in legitimate
/// article prose.
pub static NAV_TOKENS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(about us|contact us|privacy policy|terms of service|careers|pricing|blog|sign in|log in|faq|help center|get started)\b",
    )
    .expect("valid regex: nav tokens")
});

/// Minimum nav-token occurrences before text counts as nav-dense.
const NAV_TOKEN_DENSITY: usize = 3;

/// Nav-dense tokens (unambiguous nav-menu words) used as an additional
/// empty-shell signal in the 80-120 char band.
///
/// Canonical nav-density check exported for reuse (e.g. the search crate's
/// harvest pipeline): text is nav-dense when it contains at least
/// [`NAV_TOKEN_DENSITY`] unambiguous nav-menu tokens.
pub fn is_nav_dense(text: &str) -> bool {
    NAV_TOKENS.find_iter(text).count() >= NAV_TOKEN_DENSITY
}

/// Link-dense boilerplate heuristics: content that is mostly navigation
/// menu text rather than article prose. Delegates the shared nav-token count
/// to [`is_nav_dense`] and adds a read-more-laden fallback.
pub(super) fn is_nav_link_dense(text: &str) -> bool {
    // Intentional difference from `READ_MORE_INDICATOR`: this regex matches
    // *any* occurrence of "learn more"/"read more" phrases (used as a
    // link-dense boilerplate signal), whereas `READ_MORE_INDICATOR` is the
    // exact "Read More »" string that flags a paywall teaser when it is the
    // entire content. The two serve different purposes and must not be merged.
    static READ_MORE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)learn more|read more").expect("valid regex: read-more phrases")
    });
    is_nav_dense(text) || READ_MORE.find_iter(text).count() >= 3
}

/// GitHub-style SPA nav: a repo page that renders only the tab chrome
/// (Code / Issues / Pull requests / Actions / Projects / Wiki / Security /
/// Insights / Settings) with no article body.
pub(super) fn is_github_spa_nav(text: &str) -> bool {
    static GITHUB_NAV: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)\b(code|issues|pull requests|actions|projects|wiki|security|insights|settings)\b",
        )
        .expect("valid regex: github nav")
    });
    GITHUB_NAV.find_iter(text).count() >= 4
}

/// Boilerplate content: image placeholders, share/listen prompts, category
/// views, and "featured" headers.
pub(super) fn has_boilerplate(text: &str) -> bool {
    static BOILERPLATE: LazyLock<RegexSet> = LazyLock::new(|| {
        RegexSet::new([
            r"(?i)press enter or click to view image",
            r"(?i)share this (page|article|post)",
            r"(?i)listen (to|on) (podcast|episode)",
            r"(?i)^view (all|categories)",
            r"(?i)^featured",
        ])
        .expect("valid RegexSet for BOILERPLATE")
    });
    BOILERPLATE.is_match(text)
}
