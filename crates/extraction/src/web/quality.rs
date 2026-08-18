//! Quality scoring for extracted web content.
//!
//! [`WebExtractor::score_quality`] consumes the typed [`ContentQuality::validate`]
//! result, penalizes content with fewer than two sections, and clamps/rounds the
//! final score. Reasons are encoded with a stable snake_case string.

use crate::article::{QualityScore, Section};
use crate::quality::QualityResult;

use super::WebExtractor;

impl WebExtractor {
    pub(super) fn score_quality(result: &QualityResult, sections: &[Section]) -> QualityScore {
        let mut q = QualityScore::from_result(result);

        // Web-specific penalty on top of the shared validation: content with
        // fewer than two sections is penalized for lacking structure.
        if sections.len() < 2 {
            q.score -= 0.1;
            q.reasons.push("no_headings".into());
        }

        q.score = crate::article::round_score(q.score.clamp(0.0, 1.0));
        q.is_ok = q.score >= 0.5;
        q
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use crate::ContentQuality;
    use crate::article::Section;

    // ── Quality scoring ──

    #[test]
    fn test_score_quality_empty() {
        let result = ContentQuality::validate("");
        let q = WebExtractor::score_quality(&result, &[]);
        assert!(!q.is_ok);
        assert!(q.reasons.contains(&"empty_content".to_string()));
        assert!(q.reasons.contains(&"no_headings".to_string()));
    }

    #[test]
    fn test_score_quality_good() {
        let text = "A".repeat(500);
        let result = ContentQuality::validate(&text);
        let sections = vec![
            Section {
                heading: "Intro".into(),
                depth: 1,
                offset: 0,
                length: 100,
                content: "a".into(),
                subsections: vec![],
            },
            Section {
                heading: "Body".into(),
                depth: 2,
                offset: 100,
                length: 300,
                content: "b".into(),
                subsections: vec![],
            },
        ];
        let q = WebExtractor::score_quality(&result, &sections);
        assert!(q.is_ok);
        assert!(q.score > 0.5);
    }

    #[test]
    fn test_score_quality_paywall() {
        let result = ContentQuality::validate(crate::quality::READ_MORE_INDICATOR);
        let q = WebExtractor::score_quality(&result, &[]);
        assert!(!q.is_ok);
        assert!(q.reasons.contains(&"paywall_teaser".to_string()));
        assert!(q.reasons.contains(&"no_headings".to_string()));
    }
}
