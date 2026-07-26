use super::detection::{regex_has_long_words, regex_has_paragraphs, regex_has_punctuation};
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
            };
        }

        let slice = if text.len() > 15000 {
            &text[..15000]
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
        if slice == "Read More \u{00bb}" {
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
            score: crate::article::round_score(score),
            is_ok: score >= 0.5,
            reasons,
            length,
        }
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
}
