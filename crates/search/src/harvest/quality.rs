use std::sync::LazyLock;

use gthings_common::domain_reputation::QualityFlag;
use gthings_extraction::article::{QualityScore, Section};
use gthings_extraction::quality::QualityReason;
use gthings_extraction::quality::shannon_entropy;
use regex::Regex;

/// Detect nav-heavy chrome text (unambiguous nav-menu tokens repeated).
///
/// Used to distinguish genuine navigation-only pages from dense article prose:
/// a real article may mention "blog" or "pricing" once, but a page that is
/// *mostly* nav menu text repeats many of these tokens. Dense prose is never
/// nav-heavy, so it is never penalized as boilerplate or dropped as chrome.
pub(super) fn is_nav_heavy(content: &str) -> bool {
    static NAV: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)\b(about us|contact us|privacy policy|terms of service|careers|pricing|blog|sign in|log in|faq|help center|get started)\b",
        )
        .expect("valid regex: nav tokens")
    });

    let len = content.len();

    // Length guard: a long article (thousands of chars) that merely contains a
    // few nav-menu tokens is real prose, never a nav-only page. Only genuinely
    // short content can be nav-heavy, so anything large is never flagged.
    if len > 1000 {
        return false;
    }

    let count = NAV.find_iter(content).count();
    if count < 3 {
        return false;
    }

    // Short content (< 500 chars) dominated by nav tokens is a nav-only page.
    if len < 500 {
        return true;
    }

    // Medium content (500-1000 chars): nav tokens must form a large fraction of
    // the content. Each nav token is roughly 10-20 chars; require them to make
    // up a substantial share before flagging.
    count * 15 >= len / 2
}

/// Compute a [`QualityScore`] from extracted text content.
///
/// Delegates standard length/word-count checks to [`ContentQuality::validate`],
/// then applies harvest-specific overrides (skip-length for PDF/Arxiv, entropy adjustments).
///
/// When `skip_length_checks` is `true`, the too-short / too-few-words penalties are skipped
/// (used for PDF and Arxiv content where short body text is expected).
///
/// Production callers should prefer [`compute_quality_with_flags`] to reuse the
/// flags already detected by the follow path; this wrapper is retained for tests.
#[cfg(test)]
pub(super) fn compute_quality(content: &str, skip_length_checks: bool) -> QualityScore {
    let flags = gthings_extraction::ContentQuality::detect_all(content);
    compute_quality_with_flags(content, skip_length_checks, &flags)
}

/// Like [`compute_quality`], but takes precomputed quality flags.
///
/// The caller (the follow path) already runs [`ContentQuality::detect_all`]
/// once for reputation write-back; passing the flags here avoids a second
/// full-text scan of the same content.
pub(super) fn compute_quality_with_flags(
    content: &str,
    skip_length_checks: bool,
    flags: &[QualityFlag],
) -> QualityScore {
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
    for flag in flags {
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
                score -= 0.3;
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
        if v.reasons.contains(&QualityReason::Boilerplate) {
            // Only penalize clear boilerplate chrome (nav-heavy pages), never
            // dense prose that merely contains a share/listen phrase. Raw
            // article content must be preserved for AI-agent consumption.
            if is_nav_heavy(content) {
                reasons.push("boilerplate".into());
                score -= 0.1;
            }
        }
        // Navigation chrome (nav-menu text) only drops when the page is
        // genuinely nav-heavy; dense prose is never nav-heavy.
        if v.reasons.contains(&QualityReason::NavigationChrome) && is_nav_heavy(content) {
            reasons.push("nav_chrome".into());
            score -= 0.6;
        }
    }

    let entropy = shannon_entropy(content);
    // Only penalize genuinely garbled/repetitive text (very low entropy).
    // Dense but legitimate prose (entropy ~2-4) is NOT penalized, so raw
    // article content is never dropped for being repetitive.
    if entropy < 1.5 {
        reasons.push("low_entropy".into());
        score -= 0.1;
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
    fn test_compute_quality_dense_repetitive_prose_not_dropped() {
        // A dense, repetitive article with real paragraphs must NOT be dropped
        // even though it is low-entropy and contains a share/listen phrase.
        let text = "The core thesis of this report is that the market will continue to grow. \
                    The market grows because demand grows, and demand grows because adoption \
                    grows. We repeat this point many times throughout the report to emphasize \
                    the importance of growth in the market. Share this article with your \
                    colleagues and listen to our podcast for more analysis. The market grows, \
                    demand grows, adoption grows, and the report repeats these themes over and \
                    over again to make the central argument unmistakably clear to every reader.";
        let q = compute_quality(text, false);
        assert!(
            q.is_ok,
            "dense repetitive prose must not be dropped, got reasons: {:?}",
            q.reasons
        );
        assert!(q.score >= 0.5, "dense prose score should be >= 0.5, got {}", q.score);
    }

    #[test]
    fn test_compute_quality_nav_only_still_dropped() {
        // A genuinely nav-only page (repeated menu tokens, no prose) must still
        // be dropped.
        let text = "About Us Contact Us Privacy Policy Terms of Service Careers Pricing Blog \
                    Sign In Log In FAQ Help Center Get Started About Us Contact Us Privacy \
                    Policy Terms of Service Careers Pricing Blog Sign In Log In FAQ Help Center";
        let q = compute_quality(text, false);
        assert!(
            !q.is_ok,
            "nav-only page must still be dropped, got reasons: {:?}",
            q.reasons
        );
    }

    #[test]
    fn test_is_nav_heavy_length_guard() {
        // A long article (thousands of chars) containing nav-menu tokens must
        // NOT be flagged nav-heavy — it is real prose, not a nav-only page.
        let mut long = String::new();
        while long.len() < 15_000 {
            long.push_str(
                "This is a real paragraph of article prose discussing the topic at length. \
                 The blog and pricing pages are mentioned in passing, but the bulk of this \
                 text is genuine content that a reader would want to consume. ",
            );
        }
        assert!(long.len() > 1000, "test content must exceed the length guard");
        assert!(
            !is_nav_heavy(&long),
            "long article with nav tokens must not be nav-heavy"
        );

        // A short nav-only page must still be flagged.
        let short = "About Us Contact Us Privacy Policy Terms of Service Careers Pricing Blog \
                     Sign In Log In FAQ Help Center Get Started About Us Contact Us Privacy \
                     Policy Terms of Service Careers Pricing Blog Sign In Log In FAQ Help Center";
        assert!(is_nav_heavy(short), "short nav-only page must be nav-heavy");
    }

    #[test]
    fn test_compute_quality_long_article_with_nav_tokens_not_dropped() {
        // A long article (15k chars) that happens to contain nav-menu tokens
        // must be kept as Ok, never dropped as nav-heavy chrome.
        let mut long = String::new();
        while long.len() < 15_000 {
            long.push_str(
                "This is a real paragraph of article prose discussing the topic at length. \
                 The blog and pricing pages are mentioned in passing, but the bulk of this \
                 text is genuine content that a reader would want to consume. ",
            );
        }
        let q = compute_quality(&long, false);
        assert!(
            q.is_ok,
            "long article with nav tokens must be kept, got reasons: {:?}",
            q.reasons
        );
        assert!(
            !q.reasons.iter().any(|r| r.contains("nav_chrome")),
            "long article must not be flagged nav_chrome, got: {:?}",
            q.reasons
        );
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

    #[test]
    fn test_compute_quality_legitimate_prose_not_boilerplate() {
        // "share" and "listen" used as ordinary verbs in real prose must NOT
        // trigger the Boilerplate reason (no bare-verb false positives).
        let text = "The team will share the key findings with stakeholders next week. \
                    You can listen to the full briefing on our podcast feed, and then \
                    review the written summary in the report appendix. This paragraph \
                    has enough words and length to clear the shell detection heuristics.";
        let q = compute_quality(text, false);
        assert!(
            !q.reasons.iter().any(|r| r.contains("boilerplate")),
            "legitimate prose using 'share'/'listen' should not be boilerplate, got: {:?}",
            q.reasons
        );
    }

    #[test]
    fn test_compute_quality_short_valid_prose_not_empty_shell() {
        // Short-but-valid prose (80-120 chars, >= 15 words, with punctuation)
        // must NOT be flagged as an empty shell after the band fix.
        let text = "Quick brown foxes leap over lazy dogs near the river bank while patient \
                    hunters watch closely from behind tall trees.";
        let q = compute_quality(text, false);
        assert!(
            !q.reasons.iter().any(|r| r.contains("empty_shell")),
            "valid short prose should not be empty_shell, got: {:?}",
            q.reasons
        );
        assert!((q.score - 1.0).abs() < 1e-9, "score should be 1.0, got: {}", q.score);
    }

    #[test]
    fn test_compute_quality_short_no_punctuation_is_empty_shell() {
        // Short content with no punctuation (machine/nav output) still fires
        // the EmptyShell flag in the 80-120 band.
        let text = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        assert!(text.len() >= 80 && text.len() < 120);
        let q = compute_quality(text, false);
        assert!(
            q.reasons.iter().any(|r| r.contains("empty_shell")),
            "short no-punctuation content should be empty_shell, got: {:?}",
            q.reasons
        );
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
