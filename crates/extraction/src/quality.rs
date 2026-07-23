/// Content quality validation and bot/captcha/paywall detection.
///
/// Ported from `skills/cdp/scripts/quality.ts`.
/// All functions are PURE: same inputs → same outputs. No I/O, no side effects.
/// Operates on extracted text content, not DOM.
use regex::Regex;
use std::sync::OnceLock;

/// Result of a content quality validation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct QualityResult {
    /// Quality score from 0.0 to 1.0.
    pub score: f64,
    /// Whether the content passes the quality gate (score >= 0.5).
    pub is_ok: bool,
    /// List of reasons why content failed quality checks.
    pub reasons: Vec<String>,
    /// Length of the input text that was validated.
    pub length: usize,
}

/// Result of a secondary quality check.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SecondaryResult {
    /// Content ends mid-sentence without sentence-ending punctuation.
    pub truncated: bool,
    /// Content contains repetitive sentences (unique < 50% of total).
    pub repetitive: bool,
    /// Content has low word density (< 20 words).
    pub sparse: bool,
    /// Content is suspiciously short with redirect/loading patterns.
    pub suspicious_short: bool,
}

/// Content quality validation and bot/captcha/paywall detection.
///
/// All methods are stateless — they operate purely on input text and return
/// deterministic results.
pub struct ContentQuality;

impl ContentQuality {
    // Error page patterns (shared with detect methods)

    fn error_page_reasons() -> &'static [(Regex, &'static str)] {
        static PATTERNS: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
        PATTERNS.get_or_init(|| {
            vec![
                (
                    Regex::new(r"(?i)This site can't be reached").expect("valid regex"),
                    "browser_error_page",
                ),
                (Regex::new(r"ERR_CONNECTION").expect("valid regex"), "connection_error"),
                (Regex::new(r"404 Not Found").expect("valid regex"), "not_found"),
                (Regex::new(r"^\s*$").expect("valid regex"), "whitespace_only"),
            ]
        })
    }

    /// Validate extracted text content.
    ///
    /// Checks: minimum length, error page patterns, paywall teasers,
    /// navigation chrome, word count, punctuation, and natural language
    /// indicators (long words, paragraphs).
    ///
    /// Returns a [`QualityResult`] with a score from 0.0 to 1.0 and a list
    /// of failure reasons.
    ///
    /// Content is considered acceptable (`is_ok == true`) when score >= 0.5.
    ///
    /// # Examples
    ///
    /// ```
    /// # use extraction::ContentQuality;
    /// let text = "This is a sufficiently long piece of text with natural language. \
    /// It has sentences, punctuation, and enough words to pass the quality gate. \
    /// This content should be considered acceptable for AI processing.";
    /// let result = ContentQuality::validate(text);
    /// assert!(result.is_ok);
    /// ```
    pub fn validate(text: &str) -> QualityResult {
        let length = text.len();

        if text.is_empty() {
            return QualityResult {
                score: 0.0,
                is_ok: false,
                reasons: vec!["empty_content".to_string()],
                length: 0,
            };
        }

        let slice = if text.len() > 15000 {
            &text[..15000]
        } else {
            text
        };
        let mut reasons: Vec<String> = Vec::new();
        let mut score = 1.0_f64;

        // Too short to be useful (< 80 chars)
        if slice.len() < 80 {
            reasons.push("too_short".to_string());
            score -= 0.4;
        }

        // Browser error pages / empty shell (shared patterns)
        for (pattern, reason) in Self::error_page_reasons() {
            if pattern.is_match(slice) {
                reasons.push((*reason).to_string());
                score -= 0.5;
            }
        }

        // Paywall teaser: "Read More »" as entire content
        if slice == "Read More \u{00bb}" {
            reasons.push("paywall_teaser".to_string());
            score -= 0.5;
        }

        // Navigation chrome: short content with no natural language (no quotes)
        if slice.len() < 100 && !slice.contains('"') {
            reasons.push("navigation_chrome".to_string());
            score -= 0.3;
        }

        // Very few words suggests empty shell
        let word_count = slice.split_whitespace().count();
        if word_count < 15 && slice.len() < 200 {
            reasons.push("too_few_words".to_string());
            score -= 0.2;
        }

        // No punctuation suggests machine output
        if slice.len() > 100 && !regex_has_punctuation(slice) {
            reasons.push("no_punctuation".to_string());
            score -= 0.1;
        }

        // Bonus for natural language indicators
        if regex_has_punctuation(slice) {
            score += 0.05;
        }
        if regex_has_long_words(slice) {
            score += 0.05;
        }
        if regex_has_paragraphs(slice) {
            score += 0.05;
        }

        score = score.clamp(0.0, 1.0);

        QualityResult {
            score: (score * 100.0).round() / 100.0,
            is_ok: score >= 0.5,
            reasons,
            length,
        }
    }

    /// Detect bot challenge pages (Cloudflare, DataDome, Turnstile, etc.).
    ///
    /// Returns `true` if any bot challenge pattern is found in the text.
    ///
    /// # Examples
    ///
    /// ```
    /// # use extraction::ContentQuality;
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
        // Too short to have meaningful content
        if text.len() < 80 {
            return true;
        }

        // JS-required message
        static JS_RE: OnceLock<Regex> = OnceLock::new();
        let js_re = JS_RE.get_or_init(|| Regex::new(r"(?i)please enable javascript").expect("valid regex"));
        if js_re.is_match(text) {
            return true;
        }

        // Fewer than 10 words — likely navigation chrome only
        let words: Vec<&str> = text.split_whitespace().collect();
        if words.len() < 10 {
            return true;
        }

        false
    }

    /// Determine whether a URL should be recrawled with different parameters.
    ///
    /// Returns `true` when the quality score is low enough that a retry
    /// with different configuration might yield better results.
    pub fn needs_recrawl(result: &QualityResult) -> bool {
        // Very low quality — definitely retry
        if result.score < 0.3 {
            return true;
        }

        // Borderline — retry if the reason suggests a params issue
        if result.score < 0.5 {
            let retryable = ["too_short", "too_few_words", "navigation_chrome"];
            if result
                .reasons
                .iter()
                .any(|r| retryable.contains(&r.as_str()))
            {
                return true;
            }
        }

        false
    }

    /// Secondary quality check on already-cleaned content.
    ///
    /// More lenient than [`validate`] — intended for use after the primary
    /// quality gate has already filtered obvious junk. Checks for:
    ///
    /// - **Truncation**: content ends mid-sentence.
    /// - **Suspicious short**: very short with redirect/loading keywords.
    /// - **Sparse content**: fewer than 20 words.
    /// - **Repetitive content**: unique sentences < 50% of total sentences.
    pub fn secondary_check(text: &str) -> SecondaryResult {
        if text.is_empty() {
            return SecondaryResult {
                truncated: false,
                repetitive: false,
                sparse: true,
                suspicious_short: false,
            };
        }

        let mut truncated = false;
        let mut repetitive = false;
        let mut sparse = false;
        let mut suspicious_short = false;

        // Truncation detection: ends mid-sentence (last char is not sentence-ending)
        let trimmed = text.trim();
        if trimmed.len() > 100 {
            let last_char = trimmed.chars().last().unwrap_or(' ');
            let second_last = trimmed.chars().rev().nth(1).unwrap_or(' ');
            if last_char.is_ascii_alphanumeric()
                && !(second_last == '.'
                    || second_last == '!'
                    || second_last == '?'
                    || last_char == '.'
                    || last_char == '!'
                    || last_char == '?')
            {
                truncated = true;
            }
        }

        // Still too short — check for suspicious patterns
        if text.len() < 80 {
            static SUS_RE: OnceLock<Regex> = OnceLock::new();
            let sus_re = SUS_RE.get_or_init(|| {
                Regex::new(
                    "(?i)(redirect|click here|\
                     please continue|click to continue|continue to next|\
                     loading|please wait)",
                )
                .expect("valid regex")
            });
            if sus_re.is_match(text) {
                suspicious_short = true;
            }
        }

        // Low word density suggests sparse content
        let words: Vec<&str> = text.split_whitespace().collect();
        if words.len() < 20 && !text.is_empty() {
            sparse = true;
        }

        // Check for repetitive content (same sentence appearing many times)
        let sentences: Vec<&str> = text
            .split(['.', '!', '?'])
            .filter(|s| s.trim().len() > 20)
            .collect();

        if sentences.len() > 3 {
            let mut unique = std::collections::HashSet::new();
            for s in &sentences {
                unique.insert(s.trim().to_lowercase());
            }
            if unique.len() < (sentences.len() as f64 * 0.5) as usize {
                repetitive = true;
            }
        }

        SecondaryResult {
            truncated,
            repetitive,
            sparse,
            suspicious_short,
        }
    }
}

// Private helpers

/// Check if text contains sentence-ending punctuation.
fn regex_has_punctuation(text: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"[.!?]").expect("valid regex"));
    re.is_match(text)
}

/// Check if text contains words with 4+ characters.
fn regex_has_long_words(text: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"\w{4,}").expect("valid regex"));
    re.is_match(text)
}

/// Check if text contains paragraph breaks (double newline).
fn regex_has_paragraphs(text: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"\n\n").expect("valid regex"));
    re.is_match(text)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(result.reasons.contains(&"empty_content".to_string()));
    }

    #[test]
    fn test_validate_too_short() {
        let result = ContentQuality::validate("Hi");
        assert!(!result.is_ok);
        assert!(result.reasons.contains(&"too_short".to_string()));
    }

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

    #[test]
    fn test_needs_recrawl() {
        let low = QualityResult {
            score: 0.2,
            is_ok: false,
            reasons: vec!["too_short".into()],
            length: 10,
        };
        assert!(ContentQuality::needs_recrawl(&low));

        let borderline = QualityResult {
            score: 0.4,
            is_ok: false,
            reasons: vec!["too_short".into()],
            length: 50,
        };
        assert!(ContentQuality::needs_recrawl(&borderline));

        let bad_reason = QualityResult {
            score: 0.4,
            is_ok: false,
            reasons: vec!["paywall_teaser".into()],
            length: 50,
        };
        assert!(!ContentQuality::needs_recrawl(&bad_reason));

        let good = QualityResult {
            score: 0.7,
            is_ok: true,
            reasons: vec![],
            length: 1000,
        };
        assert!(!ContentQuality::needs_recrawl(&good));
    }

    #[test]
    fn test_secondary_check_truncated() {
        let result = ContentQuality::secondary_check(
            "This is a long enough sentence that does not end properly here"
                .to_string()
                .repeat(3)
                .as_str(),
        );
        // Content greater than 100 chars, last char is alphanumeric, not ending in punctuation
        assert!(result.truncated);
    }

    #[test]
    fn test_secondary_check_repetitive() {
        let text = "This is a long sentence that appears many times. This is a long sentence that appears many times. This is a long sentence that appears many times. This is a long sentence that appears many times.";
        let result = ContentQuality::secondary_check(text);
        assert!(result.repetitive);
    }

    #[test]
    fn test_secondary_check_sparse() {
        let result = ContentQuality::secondary_check("Just a few words");
        assert!(result.sparse);
    }
}
