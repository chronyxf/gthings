use gthings_common::domain_reputation::QualityFlag;
use gthings_extraction::article::{QualityScore, Section};
use gthings_extraction::quality::QualityReason;
use gthings_extraction::quality::entropy::shannon_entropy;

/// Compute a [`QualityScore`] from extracted text content.
///
/// Delegates standard length/word-count checks to [`ContentQuality::validate`],
/// then applies harvest-specific overrides (skip-length for PDF/Arxiv, entropy adjustments).
///
/// When `skip_length_checks` is `true`, the too-short / too-few-words penalties are skipped
/// (used for PDF and Arxiv content where short body text is expected).
pub(super) fn compute_quality(content: &str, skip_length_checks: bool) -> QualityScore {
    let mut reasons: Vec<String> = Vec::new();
    let mut score = 1.0_f64;

    if content.is_empty() {
        return QualityScore {
            score: 0.0,
            is_ok: false,
            reasons: vec!["empty_content".into()],
            entropy_bits_per_char: 0.0,
        };
    }

    // Bot/paywall/captcha/empty-shell detection (harvest-specific, not in validate)
    for flag in gthings_extraction::ContentQuality::detect_all(content) {
        match flag {
            QualityFlag::BotWall => {
                reasons.push("bot_blocked".into());
                score -= 0.6;
            }
            QualityFlag::Paywall => {
                reasons.push("paywall".into());
                score -= 0.6;
            }
            QualityFlag::Captcha => {
                reasons.push("captcha".into());
                score -= 0.3;
            }
            QualityFlag::EmptyShell => {
                reasons.push("empty_shell".into());
                score -= 0.2;
            }
            _ => {}
        }
    }

    // Delegate length/word-count checks to ContentQuality::validate to avoid
    // duplicating the constants (80 chars, 15 words) and heuristics.
    if !skip_length_checks {
        let v = gthings_extraction::ContentQuality::validate(content);
        if v.reasons.contains(&QualityReason::TooShort) {
            reasons.push("too_short".into());
            score -= 0.2;
        }
        if v.reasons.contains(&QualityReason::TooFewWords) {
            reasons.push("too_few_words".into());
            score -= 0.1;
        }
    }

    let entropy = shannon_entropy(content);
    if entropy < 2.0 {
        reasons.push("low_entropy".into());
        score -= 0.2;
    }

    score = score.clamp(0.0, 1.0);
    let is_ok = score >= 0.5;

    // Ensure reasons is non-empty when quality is low
    let reasons = if reasons.is_empty() && !is_ok {
        vec!["low_quality".into()]
    } else {
        reasons
    };

    QualityScore {
        score,
        is_ok,
        reasons,
        entropy_bits_per_char: entropy,
    }
}

/// Extract section-like structure from plain text content.
///
/// Uses double-newline block splitting: if a block's first line is a short
/// line that doesn't end with sentence punctuation, it's treated as a heading.
///
/// Supports two formats:
/// - **Format A** — Heading and content in the same `\n\n` block, separated
///   by a single newline: `"Heading\nContent line 1\nContent line 2"`.
/// - **Format B** — Heading and content in separate `\n\n` blocks:
///   `"Heading\n\nContent paragraph"`.
pub(super) fn extract_sections(content: &str) -> Vec<Section> {
    if content.len() < 50 {
        return Vec::new();
    }

    let mut sections = Vec::new();
    let blocks: Vec<&str> = content.split("\n\n").collect();
    let mut offset = 0;
    let mut i = 0;

    // Returns `true` if `line` looks like a section heading.
    let is_heading = |s: &str| -> bool {
        let t = s.trim();
        !t.is_empty()
            && t.len() < 100
            && !t.ends_with('.')
            && !t.ends_with('!')
            && !t.ends_with('?')
            && t.chars().filter(|&c| c == ' ').count() < 12
    };

    while i < blocks.len() {
        let raw = blocks[i];
        let block = raw.trim();
        let block_start = offset;
        offset += raw.len() + 2;

        if block.is_empty() {
            i += 1;
            continue;
        }

        let lines: Vec<&str> = block.lines().collect();

        // Format A: multi-line block with heading as first line
        if lines.len() >= 2 && is_heading(lines[0]) {
            sections.push(Section {
                heading: lines[0].trim().to_string(),
                depth: 2,
                offset: block_start,
                length: raw.len(),
                content: lines[1..].join("\n"),
                subsections: Vec::new(),
            });
            i += 1;
            continue;
        }

        // Format B: single-line heading followed by content in next block
        if lines.len() == 1 && is_heading(block) && i + 1 < blocks.len() {
            let next_raw = blocks[i + 1];
            let next_block = next_raw.trim();
            if !next_block.is_empty() {
                let next_lines: Vec<&str> = next_block.lines().collect();
                let next_is_heading =
                    next_lines.len() == 1 && next_block.len() < 100 && is_heading(next_block);

                if !next_is_heading {
                    sections.push(Section {
                        heading: block.to_string(),
                        depth: 2,
                        offset: block_start,
                        length: raw.len() + 2 + next_raw.len(),
                        content: next_block.to_string(),
                        subsections: Vec::new(),
                    });
                    // Skip the content block too
                    offset += next_raw.len() + 2;
                    i += 2;
                    continue;
                }
            }
        }

        i += 1;
    }

    sections
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Quality score tests ──────────────────────────────────────────────

    #[test]
    fn test_compute_quality_empty() {
        let q = compute_quality("", false);
        assert!(!q.is_ok);
        assert_eq!(q.score, 0.0);
    }

    #[test]
    fn test_compute_quality_good_content() {
        let text = "This is a sufficiently long piece of content with many words \
                     and sentences that should pass all quality checks without \
                     triggering any of the detection heuristics for bots, paywalls, \
                     or empty shells. It has plenty of text to be considered high \
                     quality content for our research purposes. We need at least \
                     80 characters and 15 words.";
        let q = compute_quality(text, false);
        assert!(q.is_ok);
        assert!(q.score >= 0.5);
    }

    #[test]
    fn test_compute_quality_detects_bot() {
        let text = "Checking your browser before accessing the site. Please wait while we verify you are human.";
        let q = compute_quality(text, false);
        assert!(!q.is_ok);
        assert!(q.reasons.iter().any(|r| r.contains("bot_blocked")));
    }

    #[test]
    fn test_compute_quality_detects_paywall() {
        let text = "Subscribe now to continue reading this article. You have reached your free article limit.";
        let q = compute_quality(text, false);
        assert!(!q.is_ok);
        assert!(q.reasons.iter().any(|r| r.contains("paywall")));
    }

    #[test]
    fn test_compute_quality_detects_captcha() {
        let text = "Please complete the recaptcha widget to continue.";
        let q = compute_quality(text, false);
        assert!(!q.is_ok);
        assert!(q.reasons.iter().any(|r| r.contains("captcha")));
    }

    // ── Section extraction tests ─────────────────────────────────────────

    #[test]
    fn test_extract_sections_empty() {
        let sections = extract_sections("");
        assert!(sections.is_empty());
    }

    #[test]
    fn test_extract_sections_finds_headings() {
        let text = "Introduction\n\nHere is some introductory content.\n\n\
                     Background\n\nThis section provides background information.\n\n\
                     Conclusion\n\nThe final section wraps up.";
        let sections = extract_sections(text);
        assert!(!sections.is_empty());
        let headings: Vec<&str> = sections.iter().map(|s| s.heading.as_str()).collect();
        assert!(headings.contains(&"Introduction"));
        assert!(headings.contains(&"Background"));
    }

    #[test]
    fn test_compute_quality_reasons_never_empty_when_low() {
        // Empty content → reasons has "empty_content"
        let q = compute_quality("", false);
        assert!(
            q.reasons.iter().any(|r| r == "empty_content"),
            "empty content should produce 'empty_content' reason, got: {:?}",
            q.reasons
        );

        // Bot wall → reasons has "bot_blocked"
        let bot_text = "Checking your browser before accessing the site. Please wait while we verify you are human.";
        let q = compute_quality(bot_text, false);
        assert!(
            q.reasons.iter().any(|r| r.contains("bot_blocked")),
            "bot-detected content should produce 'bot_blocked' reason, got: {:?}",
            q.reasons
        );

        // Paywall → reasons has "paywall"
        let paywall_text = "Subscribe now to continue reading this article. You have reached your free article limit.";
        let q = compute_quality(paywall_text, false);
        assert!(
            q.reasons.iter().any(|r| r.contains("paywall")),
            "paywall content should produce 'paywall' reason, got: {:?}",
            q.reasons
        );

        // Tiny content → reasons has "too_short"
        let q = compute_quality("tiny", false);
        assert!(
            q.reasons.iter().any(|r| r.contains("too_short")),
            "tiny content should produce 'too_short' reason, got: {:?}",
            q.reasons
        );
    }

    #[test]
    fn test_body_status_mapping() {
        // Empty content → ChromeOrEmpty (indirectly via quality)
        let q = compute_quality("", false);
        assert!(!q.is_ok, "empty content quality should not be ok");
        assert!(
            q.reasons.iter().any(|r| r == "empty_content"),
            "empty content should have empty_content reason"
        );

        // Good content → Ok (mapped via quality check)
        let good = "This is a sufficiently long piece of content with many words \
                     and sentences that should pass all quality checks without \
                     triggering any of the detection heuristics.";
        let q = compute_quality(good, false);
        assert!(q.is_ok, "good content quality should be ok");
    }
}
