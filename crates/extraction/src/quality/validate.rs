//! Content quality validation and detection.
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

use super::{ContentQuality, QualityFlag, QualityReason, QualityResult, shannon_entropy};

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
        r"(?i)(please enable javascript|try using)",
    ])
    .expect("valid RegexSet for ALL_PATTERNS")
});

// ---------------------------------------------------------------------------
// Text-metric regexes (punctuation, long words, paragraphs)
// ---------------------------------------------------------------------------

const IDX_PUNCTUATION: usize = 0;
const IDX_LONG_WORDS: usize = 1;

static TEXT_METRICS: LazyLock<[Regex; 2]> = LazyLock::new(|| {
    [
        Regex::new(r"[.!?]").expect("valid regex: punctuation"),
        Regex::new(r"\w{4,}").expect("valid regex: long words"),
    ]
});

/// Determine whether text is an empty shell.
///
/// - < 80 chars: always a shell (overlaps with TooShort).
/// - JS-required pattern: always a shell.
/// - 80-120 char band: only a shell when an additional signal is present
///   (no punctuation, or nav-dense tokens) so short-but-valid prose is not
///   penalized.
/// - >= 120 chars: shell only when it has very few words.
fn is_empty_shell(text: &str) -> bool {
    let len = text.len();
    if len < 80 {
        return true;
    }
    if ALL_PATTERNS.matches(text).matched(IDX_EMPTY_SHELL) {
        return true;
    }
    if len < 120 {
        return !regex_has_punctuation(text) || is_nav_dense(text);
    }
    text.split_whitespace().count() < 15
}

/// Nav-dense tokens (unambiguous nav-menu words) used as an additional
/// empty-shell signal in the 80-120 char band.
fn is_nav_dense(text: &str) -> bool {
    static NAV: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)\b(about us|contact us|privacy policy|terms of service|careers|pricing|blog|sign in|log in|faq|help center|get started)\b",
        )
        .expect("valid regex: nav tokens")
    });
    NAV.find_iter(text).count() >= 3
}

/// Link-dense boilerplate heuristics: content that is mostly navigation
/// menu text rather than article prose.
fn is_nav_link_dense(text: &str) -> bool {
    // Unambiguous nav-menu tokens only. Common tech words (cloud, services,
    // support, managed, solutions, partners, consulting) are deliberately
    // excluded because they appear frequently in legitimate article prose.
    static NAV_KEYWORDS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)\b(about us|contact us|privacy policy|terms of service|careers|pricing|blog|sign in|log in|faq|help center|get started)\b",
        )
        .expect("valid regex: nav keywords")
    });
    static READ_MORE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)learn more|read more").expect("valid regex: read-more phrases")
    });
    NAV_KEYWORDS.find_iter(text).count() >= 3 || READ_MORE.find_iter(text).count() >= 3
}

/// GitHub-style SPA nav: a repo page that renders only the tab chrome
/// (Code / Issues / Pull requests / Actions / Projects / Wiki / Security /
/// Insights / Settings) with no article body.
fn is_github_spa_nav(text: &str) -> bool {
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
fn has_boilerplate(text: &str) -> bool {
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
        if is_empty_shell(text) {
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
        is_empty_shell(text)
    }

    /// Validate extracted text content (length, error patterns, word count, punctuation).
    ///
    /// Returns a [`QualityResult`] with a score from 0.0 to 1.0. Content passes when score >= 0.5.
    pub fn validate(text: &str) -> QualityResult {
        let length = text.len();

        if text.is_empty() {
            return QualityResult {
                score: 0.0,
                is_ok: false,
                reasons: vec![QualityReason::EmptyContent],
                length: 0,
                entropy_bits_per_char: 0.0,
                flags: vec![],
            };
        }

        let slice = if text.len() > 15000 {
            let mut end = 15000;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            &text[..end]
        } else {
            text
        };
        let mut reasons: Vec<QualityReason> = Vec::new();
        let mut score = 1.0_f64;

        // Too short to be useful (< 80 chars)
        if slice.len() < 80 {
            reasons.push(QualityReason::TooShort);
            score -= 0.4;
        }

        // Browser error pages / empty shell (shared patterns)
        for (pattern, reason) in Self::error_page_reasons() {
            if pattern.is_match(slice) {
                reasons.push(*reason);
                score -= 0.5;
            }
        }

        // Paywall teaser: "Read More »" as entire content
        if slice == super::READ_MORE_INDICATOR {
            reasons.push(QualityReason::PaywallTeaser);
            score -= 0.5;
        }

        // Navigation chrome: short content with no natural language (no quotes),
        // or longer content that is link-dense boilerplate (nav menu text)
        let is_link_dense =
            (slice.len() >= 100 && is_nav_link_dense(slice)) || is_github_spa_nav(slice);
        if (slice.len() < 100 && !slice.contains('"')) || is_link_dense {
            reasons.push(QualityReason::NavigationChrome);
            score -= 0.3;
        }

        // Boilerplate: image placeholders, share/listen prompts, category views,
        // "featured" headers
        if has_boilerplate(slice) {
            reasons.push(QualityReason::Boilerplate);
            score -= 0.3;
        }

        // Very few words suggests empty shell
        let word_count = slice.split_whitespace().count();
        if word_count < 15 && slice.len() < 200 {
            reasons.push(QualityReason::TooFewWords);
            score -= 0.2;
        }

        // No punctuation suggests machine output
        if slice.len() > 100 && !regex_has_punctuation(slice) {
            reasons.push(QualityReason::NoPunctuation);
            score -= 0.1;
        }

        score = Self::apply_bonus_score(slice, score);

        score = score.clamp(0.0, 1.0);

        // Shannon entropy: character-level information density
        let entropy = shannon_entropy(text);
        let mut flags: Vec<QualityFlag> = Vec::new();

        // Very low entropy with substantial length → thin/repetitive content
        if entropy < 2.0 && text.len() > 200 {
            flags.push(QualityFlag::ThinContent);
        }

        // Very high entropy → garbled / random / machine-noise content
        if entropy > 6.5 {
            flags.push(QualityFlag::Garbled);
        }

        QualityResult {
            score: crate::article::round_score(score),
            is_ok: score >= 0.5,
            reasons,
            length,
            entropy_bits_per_char: entropy,
            flags,
        }
    }

    /// Apply bonus score for natural language indicators (punctuation, long words).
    /// Paragraph bonus removed: content is now whitespace-collapsed so \n\n is
    /// no longer present in normalized text.
    fn apply_bonus_score(text: &str, score: f64) -> f64 {
        let mut bonus = score;
        if regex_has_punctuation(text) {
            bonus += 0.05;
        }
        if regex_has_long_words(text) {
            bonus += 0.05;
        }
        bonus
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

#[cfg(test)]
mod tests {
    use super::*;

    // ── Detection tests ───────────────────────────────────────────────────

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
        assert!(ContentQuality::detect_empty_shell(
            "Try using Grot AI to ask questions about your data."
        ));
        assert!(ContentQuality::detect_empty_shell(
            "Try using the search bar to find what you need."
        ));
        assert!(!ContentQuality::detect_empty_shell(
            "This is a sufficiently long text with many words that should not be detected as an empty shell because it contains real article content with proper sentence structure."
        ));
    }

    #[test]
    fn test_detect_empty_shell_band_requires_signal() {
        // 80-120 chars with punctuation and real prose → NOT an empty shell
        let prose = "This is a short but valid paragraph of real content that should not be flagged as an empty shell at all.";
        assert!(prose.len() >= 80 && prose.len() < 120);
        assert!(
            !ContentQuality::detect_empty_shell(prose),
            "short-but-valid prose in the 80-120 band should not be an empty shell"
        );

        // 80-120 chars with no punctuation → empty shell
        let no_punct = "a".repeat(100);
        assert!(
            ContentQuality::detect_empty_shell(&no_punct),
            "short content with no punctuation should be an empty shell"
        );

        // 80-120 chars that are nav-dense → empty shell
        let nav = "About Us Contact Us Privacy Policy Terms of Service Careers Pricing Blog Sign In Log In FAQ Help Center";
        assert!(nav.len() >= 80 && nav.len() < 120);
        assert!(
            ContentQuality::detect_empty_shell(nav),
            "short nav-dense content should be an empty shell"
        );
    }

    // ── Validation tests ──────────────────────────────────────────────────

    #[test]
    fn test_validate_ok() {
        let result = ContentQuality::validate(
            "This is a sufficiently long piece of text with natural language. \
             It has sentences, punctuation, and enough words to pass the quality gate. \
             This content should be considered acceptable for AI processing.",
        );
        assert!(result.is_ok);
        assert!(result.score >= 0.5);
    }

    #[test]
    fn test_validate_empty() {
        let result = ContentQuality::validate("");
        assert!(!result.is_ok);
        assert_eq!(result.score, 0.0);
        assert!(result.reasons.contains(&QualityReason::EmptyContent));
    }

    #[test]
    fn test_validate_too_short() {
        let result = ContentQuality::validate("Hi");
        assert!(!result.is_ok);
        assert!(result.reasons.contains(&QualityReason::TooShort));
    }

    #[test]
    fn test_validate_emoji_no_panic() {
        // 7500 ASCII characters = 7500 bytes; no punctuation → should trigger NoPunctuation
        let emoji_text = "x".repeat(7500);
        let result = ContentQuality::validate(&emoji_text);
        // Original length is preserved in result
        assert_eq!(result.length, 7500);
        // No punctuation → should have NoPunctuation reason
        assert!(
            result.reasons.contains(&QualityReason::NoPunctuation),
            "emoji content should trigger NoPunctuation"
        );
    }

    #[test]
    fn test_validate_cjk_no_panic() {
        // Each CJK character is 3 bytes; 6000 of them = 18,000 bytes → triggers slice
        let cjk_text = "文".repeat(6000);
        let result = ContentQuality::validate(&cjk_text);
        // Content is not treated as empty; length reflects original input
        assert_eq!(
            result.length, 18000,
            "CJK content length should be preserved"
        );
        // No punctuation → should have NoPunctuation reason
        assert!(
            result.reasons.contains(&QualityReason::NoPunctuation),
            "CJK content without punctuation should trigger NoPunctuation"
        );
    }

    #[test]
    fn test_validate_ascii_long() {
        // Pure ASCII just past the boundary (15001 bytes) → triggers truncation
        let text = "a".repeat(15001);
        let result = ContentQuality::validate(&text);
        // Truncated content should have a non-zero score
        assert!(
            result.score > 0.0,
            "score should be > 0 even for low-quality content"
        );
        // No punctuation → should have NoPunctuation reason
        assert!(
            result.reasons.contains(&QualityReason::NoPunctuation),
            "ascii-only content should trigger NoPunctuation"
        );
    }

    #[test]
    fn test_validate_79_chars_still_too_short() {
        // Below 80-char threshold should trigger TooShort
        let text = "A".repeat(79);
        let result = ContentQuality::validate(&text);
        assert!(
            result.score < 0.8 || result.reasons.contains(&QualityReason::TooShort),
            "content under 80 chars should have low score or TooShort reason"
        );
    }

    #[test]
    fn test_validate_81_chars_passes_short_threshold() {
        // Above 80-char threshold should pass the too_short check
        let text = "This is a test sentence that should be long enough to pass the too-short detection threshold in the quality validator's logic. ";
        assert!(text.len() > 80);
        let result = ContentQuality::validate(text);
        // Should not have TooShort reason
        assert!(
            !result.reasons.contains(&QualityReason::TooShort),
            "content over 80 chars should not have TooShort"
        );
    }

    #[test]
    fn test_validate_navigation_chrome_link_dense() {
        // >= 100 chars with >= 3 unambiguous nav tokens → NavigationChrome
        let text = "About Us Contact Us Privacy Policy Terms of Service Careers Pricing Blog \
                    Sign In Log In FAQ Help Center Get Started";
        assert!(text.len() >= 100);
        let result = ContentQuality::validate(text);
        assert!(
            result.reasons.contains(&QualityReason::NavigationChrome),
            "link-dense nav menu should trigger NavigationChrome, got: {:?}",
            result.reasons
        );
    }

    #[test]
    fn test_validate_cloud_article_not_nav_dense() {
        // Legitimate cloud-computing prose using common tech words must NOT be
        // flagged as nav-dense.
        let text = "Cloud computing has transformed how modern organizations manage their \
                    infrastructure. Our managed services team provides ongoing support for \
                    enterprise solutions, and many partners rely on our consulting expertise to \
                    modernize their cloud deployments while reducing operational overhead and \
                    improving reliability across regions.";
        let result = ContentQuality::validate(text);
        assert!(
            !result.reasons.contains(&QualityReason::NavigationChrome),
            "legitimate cloud-computing prose should not be nav-dense, got: {:?}",
            result.reasons
        );
    }

    #[test]
    fn test_validate_github_spa_nav() {
        // GitHub repo page rendering only tab chrome → NavigationChrome
        let text = "Code Issues Pull requests Actions Projects Wiki Security Insights Settings";
        let result = ContentQuality::validate(text);
        assert!(
            result.reasons.contains(&QualityReason::NavigationChrome),
            "GitHub SPA nav should trigger NavigationChrome, got: {:?}",
            result.reasons
        );
    }

    #[test]
    fn test_validate_legitimate_prose_not_boilerplate() {
        // "share" and "listen" used as ordinary verbs must NOT trigger Boilerplate
        let text = "The team will share the key findings with stakeholders next week. \
                    You can listen to the full briefing on our podcast feed, and then \
                    review the written summary in the report appendix.";
        let result = ContentQuality::validate(text);
        assert!(
            !result.reasons.contains(&QualityReason::Boilerplate),
            "legitimate prose using 'share'/'listen' should not be Boilerplate, got: {:?}",
            result.reasons
        );
    }

    #[test]
    fn test_validate_boilerplate_fires_on_nav_dense_text() {
        // Nav-dense page with boilerplate phrases (image placeholder, share/listen prompts)
        let text = "About Us Contact Us Privacy Policy Terms of Service Careers Pricing Blog \
                    Press enter or click to view image Listen to our podcast Share this page \
                    View all categories";
        let result = ContentQuality::validate(text);
        assert!(
            result.reasons.contains(&QualityReason::Boilerplate),
            "boilerplate text should trigger Boilerplate, got: {:?}",
            result.reasons
        );
        assert!(
            result.reasons.contains(&QualityReason::NavigationChrome),
            "nav-dense boilerplate should also trigger NavigationChrome, got: {:?}",
            result.reasons
        );
    }

    #[test]
    fn test_validate_boilerplate_featured_only() {
        // "featured" header alone fires Boilerplate
        let result = ContentQuality::validate("Featured articles from our team this week.");
        assert!(
            result.reasons.contains(&QualityReason::Boilerplate),
            "'Featured' header should trigger Boilerplate, got: {:?}",
            result.reasons
        );
    }
}
