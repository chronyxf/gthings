//! Unit tests for the follow pipeline: quality flags, boilerplate stripping,
//! whitespace collapse, leading-title removal, and JS extraction templates.

use super::*;

// ── detect_quality_flags ──────────────────────────────────────────

#[test]
fn test_detect_quality_flags_bot() {
    let flags = detect_quality_flags("Please verify you are a human. Cloudflare challenge.");
    assert!(flags.contains(&QualityFlag::BotWall));
}

#[test]
fn test_detect_quality_flags_paywall() {
    let flags = detect_quality_flags("Subscribe to read this article. Paywall.");
    assert!(flags.contains(&QualityFlag::Paywall));
}

#[test]
fn test_detect_quality_flags_clean() {
    let flags = detect_quality_flags(
        "This is a sufficiently long piece of content with many words and sentences \
         that should pass all quality checks without triggering any of the detection \
         heuristics for blocked pages or subscription prompts.",
    );
    assert!(flags.is_empty());
}

#[test]
fn test_detect_quality_flags_empty() {
    let flags = detect_quality_flags("");
    assert!(flags.contains(&QualityFlag::EmptyShell));
}

#[test]
fn test_detect_quality_flags_captcha() {
    let flags = detect_quality_flags("reCAPTCHA verification required");
    assert!(flags.contains(&QualityFlag::Captcha));
}

// ── Boilerplate stripping ─────────────────────────────────────────

#[test]
fn test_strip_boilerplate_image_caption() {
    let out = strip_boilerplate(
        "The picture above is great. Press enter or click to view image in full size. Keep reading.",
    );
    assert!(!out.to_lowercase().contains("view image in full size"));
    assert!(out.contains("The picture above is great."));
    assert!(out.contains("Keep reading."));
}

#[test]
fn test_strip_boilerplate_image_caption_variant() {
    let out = strip_boilerplate(
        "Press enter or click to view the image in full size, it shows every detail.",
    );
    assert!(!out.to_lowercase().contains("full size"));
    assert!(out.contains("it shows every detail"));
}

#[test]
fn test_strip_boilerplate_listen_share_kept() {
    // "Listen Share" is real prose and must NOT be stripped.
    assert_eq!(strip_boilerplate("Listen Share"), "Listen Share");
    let out = strip_boilerplate("Listen Share Introduction Text");
    assert_eq!(
        out.split_whitespace().collect::<Vec<_>>(),
        vec!["Listen", "Share", "Introduction", "Text"]
    );
}

#[test]
fn test_strip_boilerplate_listen_share_separators_kept() {
    let out = strip_boilerplate("Listen · Share Some article text");
    assert_eq!(
        out.split_whitespace().collect::<Vec<_>>(),
        vec!["Listen", "·", "Share", "Some", "article", "text"]
    );
    let out = strip_boilerplate("Listen | Share Body starts here");
    assert_eq!(
        out.split_whitespace().collect::<Vec<_>>(),
        vec!["Listen", "|", "Share", "Body", "starts", "here"]
    );
}

#[test]
fn test_strip_boilerplate_leading_featured_kept() {
    // A leading "Featured" label can be part of real content and must be kept.
    let out = strip_boilerplate("Featured The latest news roundup");
    assert_eq!(
        out.split_whitespace().collect::<Vec<_>>(),
        vec!["Featured", "The", "latest", "news", "roundup"]
    );
    let out = strip_boilerplate("Featured: Today in tech");
    assert_eq!(
        out.split_whitespace().collect::<Vec<_>>(),
        vec!["Featured:", "Today", "in", "tech"]
    );
}

#[test]
fn test_strip_boilerplate_featured_mid_prose_kept() {
    let content = "The site featured our article prominently";
    assert_eq!(strip_boilerplate(content), content);
}

#[test]
fn test_strip_boilerplate_nav_phrases_kept() {
    // "View Categories" / "View All Learning Resources" are real prose and
    // must NOT be stripped.
    let out = strip_boilerplate("View Categories View All Learning Resources The article body");
    assert!(out.to_lowercase().contains("view categories"));
    assert!(out.to_lowercase().contains("view all learning resources"));
    assert!(out.contains("The article body"));
}

#[test]
fn test_strip_boilerplate_normal_prose_untouched() {
    let prose = "This article explains the share economy and how to listen \
                 to featured podcasts. Press enter to continue reading.";
    assert_eq!(strip_boilerplate(prose), prose);
}

#[test]
fn test_strip_boilerplate_no_double_spaces() {
    let out = strip_boilerplate("Lead in Listen Share view categories trailing");
    assert!(!out.contains("  "), "no double spaces: {out:?}");
}

#[test]
fn test_strip_boilerplate_regex_only_image_chrome_case_insensitive() {
    // Only the unambiguous image-view chrome is stripped; all other phrases
    // are preserved as real prose.
    let out = strip_boilerplate(
        "PRESS ENTER OR CLICK TO VIEW IMAGE IN FULL SIZE Listen · Share \
         VIEW CATEGORIES view all learning resources Body text",
    );
    assert!(!out.to_lowercase().contains("view image in full size"));
    assert!(out.to_lowercase().contains("listen · share"));
    assert!(out.to_lowercase().contains("view categories"));
    assert!(out.to_lowercase().contains("view all learning resources"));
    assert!(out.contains("Body text"));
}

#[test]
fn test_strip_boilerplate_regex_the_variant() {
    let out = strip_boilerplate(
        "Press Enter or Click to View the Image in Full Size Listen | Share Intro",
    );
    assert!(!out.to_lowercase().contains("full size"));
    assert!(out.contains("Listen | Share"));
    assert!(out.contains("Intro"));
}

// ── Paragraph preservation ────────────────────────────────────────

#[test]
fn test_collapse_whitespace_preserves_paragraphs() {
    let content = "First paragraph line one.\n\nSecond paragraph.\n\n\nThird.";
    let out = collapse_whitespace(content);
    // Newlines are kept as paragraph breaks (2+ collapse to one).
    assert_eq!(out, "First paragraph line one.\nSecond paragraph.\nThird.");
}

#[test]
fn test_collapse_whitespace_collapses_inline_spaces() {
    let content = "Line   with\t\ttabs   and  spaces\nNext line";
    let out = collapse_whitespace(content);
    assert_eq!(out, "Line with tabs and spaces\nNext line");
}

#[test]
fn test_strip_boilerplate_preserves_paragraphs() {
    let content =
        "Intro paragraph.\n\nPress enter or click to view image in full size.\n\nBody paragraph.";
    let out = strip_boilerplate(content);
    assert!(!out.to_lowercase().contains("view image in full size"));
    // Paragraph breaks (newlines) are preserved, not collapsed to one line.
    assert!(out.contains('\n'), "newlines must be preserved: {out:?}");
    assert!(out.contains("Intro paragraph."));
    assert!(out.contains("Body paragraph."));
}

// ── Leading title strip ───────────────────────────────────────────

#[test]
fn test_strip_leading_title_removes_duplicate() {
    let out = strip_leading_title("My Great Article Hello world body", "My Great Article");
    assert_eq!(out, "Hello world body");
}

#[test]
fn test_strip_leading_title_case_insensitive() {
    let out = strip_leading_title("MY GREAT ARTICLE body text", "My Great Article");
    assert_eq!(out, "body text");
}

#[test]
fn test_strip_leading_title_no_match_untouched() {
    let content = "A completely different opening paragraph";
    assert_eq!(strip_leading_title(content, "Some Title"), content);
}

#[test]
fn test_strip_leading_title_empty_title() {
    let content = "Just body text";
    assert_eq!(strip_leading_title(content, ""), content);
    assert_eq!(strip_leading_title(content, "   "), content);
}

// ── FollowResult JSON parsing ──────────────────────────────────────

#[test]
fn test_follow_result_parse_valid() {
    let json = r#"{"title":"Hello","content":"World","error":""}"#;
    let result: FollowResult = serde_json::from_str(json).unwrap();
    assert_eq!(result.title, "Hello");
    assert_eq!(result.content, "World");
    assert!(result.error.is_empty());
}

#[test]
fn test_follow_result_parse_with_error() {
    let json = r#"{"title":"","content":"","error":"content too short (3 chars)"}"#;
    let result: FollowResult = serde_json::from_str(json).unwrap();
    assert!(result.title.is_empty());
    assert!(result.content.is_empty());
    assert_eq!(result.error, "content too short (3 chars)");
}

#[test]
fn test_follow_result_parse_missing_fields() {
    // Missing fields should get serde defaults (empty strings).
    let json = r#"{}"#;
    let result: FollowResult = serde_json::from_str(json).unwrap();
    assert!(result.title.is_empty());
    assert!(result.content.is_empty());
    assert!(result.error.is_empty());
}

#[test]
fn test_follow_result_parse_malformed() {
    let json = r#"not valid json"#;
    let err = serde_json::from_str::<FollowResult>(json);
    let _ = err.unwrap_err();
}

#[test]
fn test_follow_result_parse_partial() {
    // Only title provided; content/error should be empty.
    let json = r#"{"title":"Partial"}"#;
    let result: FollowResult = serde_json::from_str(json).unwrap();
    assert_eq!(result.title, "Partial");
    assert!(result.content.is_empty());
    assert!(result.error.is_empty());
}
