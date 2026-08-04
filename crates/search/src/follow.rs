//! Page following and content extraction via CDP.
//!
//! Navigates to a URL, waits for network idle, then extracts title and body
//! text via in-browser JavaScript evaluation.

use std::sync::OnceLock;
use std::time::Instant;

use chrono::Utc;
use gthings_cdp::{CdpError, Session, Tab};
use gthings_common::domain_reputation::{DomainReputation, QualityFlag};
use gthings_common::pagination::{ExtractParams, Pagination};
use gthings_common::provenance::{ExtractionMethod, Provenance};
use regex::Regex;
use serde::{Deserialize, Serialize};

/// Result of following a URL and extracting its content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FollowResult {
    /// The URL that was followed.
    #[serde(default)]
    pub url: String,
    /// Document title from `<title>`.
    #[serde(default)]
    pub title: String,
    /// Extracted page text (up to `max_chars`).
    #[serde(default)]
    pub content: String,
    /// Non-empty if the in-browser JS evaluation threw an error.
    #[serde(default)]
    pub error: String,
    /// How and when this content was acquired.
    #[serde(default)]
    pub provenance: Provenance,
    /// Pagination state.
    pub pagination: Option<Pagination>,
    /// Quality flags detected on the extracted content (single scan, shared
    /// with the reputation write-back and downstream quality scoring).
    #[serde(default)]
    pub quality_flags: Vec<QualityFlag>,
}

/// Follow a URL and extract page content via CDP.
///
/// Navigates to the URL, waits for network idle via lifecycle events,
/// then extracts the page title and body text (up to `max_chars`) via
/// in-browser JS evaluation.
///
/// # Arguments
///
/// * `session` — The CDP session managing the browser connection.
/// * `tab` — An already-created tab. Will be navigated to `url`.
/// * `url` — The URL to fetch.
/// * `params` — Extraction parameters (offset, max_chars).
/// * `reputation` — Optional domain reputation cache. When provided, the
///   function checks the domain's reputation before navigating; if the
///   domain is blocked (BotWall/Paywall on 2+ consecutive hits), it
///   returns a synthesized low-quality result without CDP navigation.
///   After extraction, detected quality flags are written back.
pub async fn follow(
    session: &Session,
    tab: &Tab,
    url: &str,
    params: ExtractParams,
    reputation: Option<&DomainReputation>,
) -> Result<FollowResult, CdpError> {
    let start = Instant::now();
    let host = gthings_common::extract_host(url).unwrap_or_else(|| {
        tracing::warn!("follow: failed to parse host from URL: {url}");
        String::new()
    });

    // ── Early-exit if domain reputation says blocked ──
    if let Some(rep) = reputation {
        if !host.is_empty() && rep.is_blocked(&host).await {
            let duration_ms = start.elapsed().as_millis() as u64;
            return Ok(make_error_result(
                url,
                "blocked by domain reputation (BotWall/Paywall)",
                duration_ms,
            ));
        }
    }

    // ── Real CDP extraction ──
    tab.navigate(session, url).await?;

    // ── In-browser pre-check (before full extraction) ──
    // Runs a lightweight JS snippet (< 50ms) to detect bot-walls, captchas,
    // and paywalls early. If any quality flags are found, skip the expensive
    // extraction JS and return immediately with those flags.
    if let Ok(flags) = session.check_page_signals(tab).await {
        let has_blockers = flags.iter().any(gthings_common::quality_flag_is_blocking);
        if has_blockers {
            // Write flags into reputation cache so future requests skip this domain
            if let Some(rep) = reputation {
                if !host.is_empty() {
                    rep.write(&host, &flags).await;
                }
            }
            let duration_ms = start.elapsed().as_millis() as u64;
            return Ok(make_error_result(
                url,
                &format!("early abort: blocked by {:?}", flags),
                duration_ms,
            ));
        }
    }

    // In-browser JS: extract page content with SPA rendering tolerance.
    // 1) Async polling loop (up to 3 s) with 100 ms yields — does NOT block
    //    the main thread, so SPAs can render.
    // 2) Prefer <main>, <article>, or [role="main"] over <body>.
    // 3) Conditional stripping: remove chrome elements only when a semantic
    //    container was found; when falling back to <body> keep nav/footer/header.
    // 4) Fallback from innerText to textContent when extracted text is < 80 chars
    //    (catches SPAs that render content via CSS-displayed elements).
    // 5) Wrapped in an async IIFE so CDP with awaitPromise:true waits for it.
    let js = include_str!("../templates/follow_extract.js")
        .replace("__OFFSET__", &params.offset.to_string())
        .replace("__MAX_CHARS__", &params.max_chars.to_string());

    let result = tab.evaluate(session, &js).await?;
    let raw = result.pointer("/result/value").and_then(|v| v.as_str());
    let json_str = raw.unwrap_or_else(|| {
        tracing::warn!("follow: CDP result missing /result/value field");
        r#"{"title":"","content":"","error":"CDP result missing value field"}"#
    });
    let mut follow_result: FollowResult = parse_follow_json(json_str).map_err(|e| *e)?;
    follow_result.url = url.to_string();
    // Post-extraction content cleaning:
    // 1) Strip only unambiguous image-view chrome (Medium image captions).
    //    Real prose is preserved verbatim — no boilerplate phrases that could
    //    appear in genuine article text are removed.
    // 2) Collapse whitespace while preserving paragraph structure: runs of
    //    spaces/tabs within a line become a single space, but newlines are
    //    kept as paragraph breaks (2+ consecutive newlines collapse to one).
    // 3) Remove a leading run of title text duplicated at the top of the body.
    let cleaned = strip_boilerplate(&follow_result.content);
    let cleaned = strip_leading_title(&cleaned, &follow_result.title);
    follow_result.content = cleaned;

    let duration_ms = start.elapsed().as_millis() as u64;
    let content_len = follow_result.content.len();

    match gthings_common::pagination::build_pagination(&params, url, content_len, content_len) {
        Ok(p) => follow_result.pagination = Some(p),
        Err(e) => tracing::warn!("failed to build pagination: {e}"),
    }

    let prov = &mut follow_result.provenance;
    prov.source_url = follow_result.url.clone();
    prov.agent = gthings_common::GTHINGS_AGENT.into();
    prov.accessed_at = Utc::now();
    prov.duration_ms = duration_ms;

    // ── Post-extraction: detect quality flags ONCE and share the result ──
    // A single full-text scan feeds both the reputation write-back below and
    // the downstream quality scoring (via `FollowResult::quality_flags`).
    let detected = detect_quality_flags(&follow_result.content);
    follow_result.quality_flags = detected.clone();
    if let Some(rep) = reputation {
        if !host.is_empty() {
            if !detected.is_empty() {
                rep.write(&host, &detected).await;
            } else {
                // Clean extraction — reset any BotWall/Paywall flags
                rep.decay(&host).await;
            }
        }
    }

    Ok(follow_result)
}

/// Parse the JSON string returned by the in-browser extraction JS,
/// returning a [`FollowResult`] or a boxed [`CdpError::Json`] on failure.
#[allow(clippy::incompatible_msrv)]
fn parse_follow_json(json_str: &str) -> Result<FollowResult, Box<CdpError>> {
    serde_json::from_str(json_str).map_err(|e| {
        let preview = &json_str[..json_str.floor_char_boundary(json_str.len().min(200))];
        tracing::warn!("follow: failed to parse extraction JSON: {e} (preview: {preview:?})");
        Box::new(CdpError::Json(e))
    })
}

/// Run quality heuristics on extracted content and return matching flags.
fn detect_quality_flags(content: &str) -> Vec<QualityFlag> {
    gthings_extraction::ContentQuality::detect_all(content)
}

/// Boilerplate phrases removed from extracted content (case-insensitively,
/// wherever they appear). Only unambiguous image-view chrome is stripped —
/// phrases that could appear in genuine article prose (e.g. "view categories",
/// "listen share") are deliberately NOT included so raw content is preserved.
const BOILERPLATE_PHRASES: [&str; 2] = [
    "press enter or click to view image in full size",
    "press enter or click to view the image in full size",
];

/// Compile the boilerplate phrases into a single case-insensitive regex.
///
/// This replaces the previous per-phrase `find_ci` O(n*m) loop (which re-scanned
/// the whole content for each of the 7 phrases) with one regex traversal over
/// the content, avoiding O(phrases * n) rescans.
fn boilerplate_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let pattern = BOILERPLATE_PHRASES
            .iter()
            .map(|p| regex::escape(p))
            .collect::<Vec<_>>()
            .join("|");
        Regex::new(&format!("(?i){pattern}")).expect("valid boilerplate regex")
    })
}

/// Strip known boilerplate noise phrases from extracted page content.
///
/// Removes, case-insensitively, only unambiguous image-view chrome: Medium
/// image captions ("Press enter or click to view image in full size", plus
/// the "view the image" variant). Real prose is preserved verbatim. Resulting
/// whitespace is collapsed while preserving paragraph structure (newlines are
/// kept as paragraph breaks).
fn strip_boilerplate(content: &str) -> String {
    let out = boilerplate_regex().replace_all(content, "");
    collapse_whitespace(&out)
}

/// Collapse runs of spaces/tabs within a line to a single space, but keep
/// newlines as paragraph breaks (2+ consecutive newlines collapse to one).
/// This preserves readable paragraph structure instead of producing one dense
/// single-line blob.
fn collapse_whitespace(content: &str) -> String {
    static SPACES: OnceLock<Regex> = OnceLock::new();
    static NEWLINES: OnceLock<Regex> = OnceLock::new();
    let spaces = SPACES.get_or_init(|| Regex::new(r"[ \t]+").expect("valid spaces regex"));
    let newlines = NEWLINES.get_or_init(|| Regex::new(r"\n{2,}").expect("valid newlines regex"));
    let out = spaces.replace_all(content, " ");
    newlines.replace_all(&out, "\n").trim().to_string()
}

/// Remove a leading run of title text from the content.
///
/// Extracted pages frequently repeat the `<title>` at the top of the body
/// (e.g. an `<h1>` mirroring it). When the content begins with text equal —
/// case-insensitively, after trimming — to the search-result title, that
/// leading run is removed.
fn strip_leading_title(content: &str, title: &str) -> String {
    let title = title.trim();
    if title.is_empty() {
        return content.to_string();
    }
    match find_ci(content, title) {
        Some((0, end)) => content[end..].trim_start().to_string(),
        _ => content.to_string(),
    }
}

/// Case-insensitive substring search.
///
/// Returns the byte range `(start, end)` of the first occurrence of `needle`
/// in `haystack`, or `None`. Matching compares per-character lowercase forms,
/// so returned offsets always refer to the original string.
fn find_ci(haystack: &str, needle: &str) -> Option<(usize, usize)> {
    if needle.is_empty() {
        return Some((0, 0));
    }
    let needle_lower: Vec<char> = needle.to_lowercase().chars().collect();
    // (original byte index, lowercase char, original char)
    let hay_lower: Vec<(usize, char, char)> = haystack
        .char_indices()
        .map(|(idx, c)| (idx, c.to_lowercase().next().unwrap_or(c), c))
        .collect();
    let n = hay_lower.len();
    let m = needle_lower.len();
    if m > n {
        return None;
    }
    'outer: for i in 0..=(n - m) {
        for j in 0..m {
            if hay_lower[i + j].1 != needle_lower[j] {
                continue 'outer;
            }
        }
        let start = hay_lower[i].0;
        let end = hay_lower[i + m - 1].0 + hay_lower[i + m - 1].2.len_utf8();
        return Some((start, end));
    }
    None
}

/// Outcome of a timed search inside a temporary tab.
pub(crate) enum TimedSearchOutcome {
    /// Search succeeded.
    Success(Vec<crate::SearchResult>),
    /// Search returned an error.
    Error(CdpError),
    /// Search timed out.
    Timeout,
}

/// Build a boilerplate [`FollowResult`] for early-exit error paths.
pub(crate) fn make_error_result(url: &str, error: &str, duration_ms: u64) -> FollowResult {
    FollowResult {
        url: url.to_string(),
        title: String::new(),
        content: String::new(),
        error: error.to_string(),
        provenance: error_provenance(url, duration_ms),
        pagination: None,
        quality_flags: Vec::new(),
    }
}

/// Build the [`Provenance`] shared by all error-result paths.
///
/// Extracted so both `follow.rs` and `harvest/orchestrator.rs` construct the
/// error provenance identically instead of duplicating the field list.
pub(crate) fn error_provenance(url: &str, duration_ms: u64) -> Provenance {
    Provenance {
        source_url: url.to_string(),
        method: ExtractionMethod::Follow,
        agent: gthings_common::GTHINGS_AGENT.into(),
        accessed_at: Utc::now(),
        duration_ms,
        derived_from: None,
    }
}

#[cfg(test)]
mod tests {
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
        let content = "Intro paragraph.\n\nPress enter or click to view image in full size.\n\nBody paragraph.";
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

    // ── Reputation early-exit (pure host-extraction check) ────────────

    #[test]
    fn test_reputation_check_blocks_follow() {
        // Verify that the URL-to-host extraction used by the block check is correct.
        // The follow() function uses gthings_common::extract_host() before checking
        // reputation.is_blocked().
        let url = "https://blocked-domain.example.com/page";
        let host = gthings_common::extract_host(url);
        assert_eq!(host, Some("blocked-domain.example.com".into()));
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

    // ── JS extraction format string smoke tests ───────────────────────

    /// Verify the format string contains the new compound selector.
    #[test]
    fn test_extraction_js_has_new_selectors() {
        let js = include_str!("../templates/follow_extract.js")
            .replace("__OFFSET__", "0")
            .replace("__MAX_CHARS__", "5000");
        assert!(
            js.contains(r#"querySelector('main, article, [role="main"]')"#),
            "JS must use the new compound selector"
        );
        assert!(
            js.contains("Date.now() < _deadline"),
            "JS must contain the 3-second async polling loop guard"
        );
        assert!(
            js.contains("await new Promise"),
            "JS must contain async/await in polling loop"
        );
        assert!(
            js.contains("_cl.textContent"),
            "JS must have textContent fallback"
        );
    }

    /// Verify that the chrome stripping query removes nav/footer/header.
    /// The stripping is consolidated into a single query applied to the
    /// cloned container regardless of branch.
    #[test]
    fn test_extraction_js_conditional_stripping_main() {
        let js = include_str!("../templates/follow_extract.js")
            .replace("__OFFSET__", "0")
            .replace("__MAX_CHARS__", "5000");
        // The stripping query must include nav, footer, header.
        assert!(
            js.contains("script,style,noscript,svg,iframe,nav,footer,header"),
            "JS must strip nav, footer, header"
        );
        // The chrome-rich query appears once (consolidated single stripping).
        let with_chrome = js.match_indices("nav,footer,header").count();
        let minimal = js.match_indices("script,style,noscript,svg,iframe").count();
        assert_eq!(
            with_chrome, 1,
            "nav,footer,header should appear once (consolidated stripping query)"
        );
        assert_eq!(
            minimal, 1,
            "the minimal stripping query should appear once (consolidated)"
        );
    }

    /// Verify the JS walks the DOM and inserts newlines at block-element
    /// boundaries so JS-rendered pages keep their paragraph breaks.
    #[test]
    fn test_extraction_js_block_boundary_newlines() {
        let js = include_str!("../templates/follow_extract.js")
            .replace("__OFFSET__", "0")
            .replace("__MAX_CHARS__", "5000");
        // The DOM-walk must exist and append a newline after block elements.
        assert!(
            js.contains("function _extractText(root)"),
            "JS must define a DOM-walk extraction function"
        );
        assert!(
            js.contains("_out.push('\\n')"),
            "JS must insert a newline after block-level elements"
        );
        // Every required block element must be handled.
        for tag in [
            "p",
            "div",
            "h1",
            "h2",
            "h3",
            "h4",
            "h5",
            "h6",
            "li",
            "br",
            "section",
            "article",
            "blockquote",
            "pre",
            "td",
            "tr",
            "ul",
            "ol",
            "table",
        ] {
            assert!(
                js.contains(&format!("'{}'", tag)),
                "JS must treat <{}> as a block element that gets a newline",
                tag
            );
        }
        // Whitespace normalization must preserve newlines (collapse 2+ to one),
        // never collapse everything to a single line.
        assert!(
            js.contains(r"replace(/\n{2,}/g, '\n')"),
            "JS must collapse 2+ newlines to a single paragraph break"
        );
    }

    /// Verify that < 3 char content produces an error in the JS logic.
    #[test]
    fn test_extraction_js_short_content_error() {
        let js = include_str!("../templates/follow_extract.js")
            .replace("__OFFSET__", "0")
            .replace("__MAX_CHARS__", "5000");
        assert!(
            js.contains("content too short ("),
            "Short content must produce an error message"
        );
        assert!(
            js.contains("_text.length < 3"),
            "The short-content threshold must be 3"
        );
    }
}
