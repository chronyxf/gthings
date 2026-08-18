//! Content quality scoring pipeline: [`ContentQuality::validate`] and the
//! text-metric helpers it relies on.

use super::patterns::{
    has_boilerplate, is_github_spa_nav, is_nav_link_dense, regex_has_long_words,
    regex_has_punctuation,
};
use super::{
    BONUS_LONG_WORDS, BONUS_PUNCTUATION, ContentQuality, MIN_CONTENT_LEN, MIN_WORDS, QualityReason,
    QualityResult, READ_MORE_INDICATOR, penalty_for, shannon_entropy,
};

impl ContentQuality {
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

        // Too short to be useful
        if slice.len() < MIN_CONTENT_LEN {
            reasons.push(QualityReason::TooShort);
        }

        // Browser error pages / empty shell (shared patterns)
        for (pattern, reason) in Self::error_page_reasons() {
            if pattern.is_match(slice) {
                reasons.push(*reason);
            }
        }

        // Paywall teaser: "Read More »" as entire content
        if slice == READ_MORE_INDICATOR {
            reasons.push(QualityReason::PaywallTeaser);
        }

        // Navigation chrome: short content with no natural language (no quotes),
        // or longer content that is link-dense boilerplate (nav menu text)
        let is_link_dense =
            (slice.len() >= 100 && is_nav_link_dense(slice)) || is_github_spa_nav(slice);
        if (slice.len() < 100 && !slice.contains('"')) || is_link_dense {
            reasons.push(QualityReason::NavigationChrome);
        }

        // Boilerplate: image placeholders, share/listen prompts, category views,
        // "featured" headers
        if has_boilerplate(slice) {
            reasons.push(QualityReason::Boilerplate);
        }

        // Very few words suggests empty shell
        let word_count = slice.split_whitespace().count();
        if word_count < MIN_WORDS && slice.len() < 200 {
            reasons.push(QualityReason::TooFewWords);
        }

        // No punctuation suggests machine output
        if slice.len() > 100 && !regex_has_punctuation(slice) {
            reasons.push(QualityReason::NoPunctuation);
        }

        // Apply penalties once from the shared (reason -> penalty) table.
        let mut score = 1.0_f64;
        for &reason in &reasons {
            score -= penalty_for(reason);
        }

        // Bonus for natural-language indicators (punctuation, long words).
        // The +0.05 bonus is intentionally smaller than the -0.1
        // NoPunctuation penalty: the bonus applies to *all* content while the
        // penalty only fires on text longer than 100 chars.
        if regex_has_punctuation(slice) {
            score += BONUS_PUNCTUATION;
        }
        if regex_has_long_words(slice) {
            score += BONUS_LONG_WORDS;
        }

        score = score.clamp(0.0, 1.0);
        let rounded = crate::article::round_score(score);

        // Shannon entropy: character-level information density
        let entropy = shannon_entropy(text);

        QualityResult {
            score: rounded,
            is_ok: rounded >= 0.5,
            reasons,
            length,
            entropy_bits_per_char: entropy,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // Real multi-byte emoji (4 bytes each) must not panic during the
        // char-boundary truncation or entropy computation.
        let emoji_text = "🙂".repeat(7500);
        let result = ContentQuality::validate(&emoji_text);
        // Original byte length is preserved in result
        assert_eq!(result.length, emoji_text.len());
        assert!((0.0..=1.0).contains(&result.score));
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
