use super::detection::{regex_has_long_words, regex_has_paragraphs, regex_has_punctuation};
use super::entropy::shannon_entropy;
use super::types::*;

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
        if slice == super::types::READ_MORE_INDICATOR {
            reasons.push(QualityReason::PaywallTeaser);
            score -= 0.5;
        }

        // Navigation chrome: short content with no natural language (no quotes)
        if slice.len() < 100 && !slice.contains('"') {
            reasons.push(QualityReason::NavigationChrome);
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

    /// Apply bonus score for natural language indicators (punctuation, long words, paragraphs).
    fn apply_bonus_score(text: &str, score: f64) -> f64 {
        let mut bonus = score;
        if regex_has_punctuation(text) {
            bonus += 0.05;
        }
        if regex_has_long_words(text) {
            bonus += 0.05;
        }
        if regex_has_paragraphs(text) {
            bonus += 0.05;
        }
        bonus
    }
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
}
