//! Content cleaning helpers for extracted page text.
//!
//! Boilerplate phrase stripping, whitespace collapse, and leading-title
//! removal shared by [`crate::follow::follow`].

use std::sync::OnceLock;

use regex::Regex;

/// Boilerplate phrases removed from extracted content (case-insensitively,
/// wherever they appear). Only unambiguous image-view chrome is stripped —
/// phrases that could appear in genuine article prose (e.g. "view categories",
/// "listen share") are deliberately NOT included so raw content is preserved.
pub(crate) const BOILERPLATE_PHRASES: [&str; 2] = [
    "press enter or click to view image in full size",
    "press enter or click to view the image in full size",
];

/// Compile the boilerplate phrases into a single case-insensitive regex.
///
/// This replaces the previous per-phrase `find_ci` O(n*m) loop (which re-scanned
/// the whole content for each of the 7 phrases) with one regex traversal over
/// the content, avoiding O(phrases * n) rescans.
pub(crate) fn boilerplate_regex() -> &'static Regex {
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
pub(crate) fn strip_boilerplate(content: &str) -> String {
    let out = boilerplate_regex().replace_all(content, "");
    collapse_whitespace(&out)
}

/// Collapse runs of spaces/tabs within a line to a single space, but keep
/// newlines as paragraph breaks (2+ consecutive newlines collapse to one).
/// This preserves readable paragraph structure instead of producing one dense
/// single-line blob.
pub(crate) fn collapse_whitespace(content: &str) -> String {
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
pub(crate) fn strip_leading_title(content: &str, title: &str) -> String {
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
pub(crate) fn find_ci(haystack: &str, needle: &str) -> Option<(usize, usize)> {
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
