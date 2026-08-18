// ---------------------------------------------------------------------------
// URL canonicalization and dedup keys
// ---------------------------------------------------------------------------

use std::borrow::Cow;

use url::Url;

use super::tracking::{build_query_string, is_tracking_param};
use super::try_parse_url;

/// Normalise a URL path:
/// - lowercase (ASCII-only — safe for URL paths)
/// - collapse `//` → `/`
/// - strip trailing `/` (except root `/`)
///
/// Single-pass, single-allocation: writes directly into a `String` buffer,
/// avoiding intermediate `Vec<&str>` and `join`/`format!` overhead.
fn normalize_path(path: &str) -> String {
    let mut result = String::with_capacity(path.len());
    let mut first = true;
    for segment in path.split('/') {
        if !segment.is_empty() {
            if !first {
                result.push('/');
            }
            first = false;
            for c in segment.chars() {
                result.push(c.to_ascii_lowercase());
            }
        }
    }
    if first {
        result.push('/');
    }
    result
}

/// Lowercase host, normalise path, strip tracking params, sort remaining
/// query parameters — all in-place on a mutable [`Url`].
fn canonicalize_parsed_url(parsed: &mut Url) {
    // Lowercase host (ASCII — safe for DNS names).
    if let Some(host) = parsed.host_str() {
        let lower_host = host.to_ascii_lowercase();
        // Note: set_host always succeeds because lower_host comes from the parsed URL's own host_str
        let _ = parsed.set_host(Some(&lower_host));
    }

    // Normalise path
    let normalized_path = normalize_path(parsed.path());
    parsed.set_path(&normalized_path);

    // Strip tracking params and sort remaining query params.
    // Keep `Cow<str>` to avoid cloning until we build the final query string.
    let mut pairs: Vec<(Cow<'_, str>, Cow<'_, str>)> = parsed
        .query_pairs()
        .filter(|(k, _)| !is_tracking_param(k))
        .collect();
    pairs.sort_by(|a, b| a.0.cmp(&b.0));

    if pairs.is_empty() {
        parsed.set_query(None);
    } else {
        let query_str = build_query_string(&pairs);
        parsed.set_query(Some(&query_str));
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Canonicalize a URL, optionally preserving its fragment.
///
/// - Lowercase scheme + host
/// - Strip tracking parameters
/// - Lowercase path
/// - Strip trailing slash (except for root `/`)
/// - Collapse `//` → `/` in path
/// - Sort remaining query parameters alphabetically
fn canonicalize(url: &str, keep_fragment: bool) -> String {
    let Some(mut parsed) = try_parse_url(url) else {
        return url.to_string();
    };

    canonicalize_parsed_url(&mut parsed);

    if !keep_fragment {
        parsed.set_fragment(None);
    }

    parsed.as_str().to_string()
}

/// Fully canonicalize a URL for human display.
///
/// Like [`canonicalize`] but the fragment is **preserved** (for display).
#[must_use]
pub fn canonicalize_url(url: &str) -> String {
    canonicalize(url, true)
}

/// Generate a deterministic dedup key (for comparison / dedup purposes).
///
/// Like [`canonicalize_url`] but the fragment is **always** stripped.
#[must_use]
pub fn dedup_key(url: &str) -> String {
    canonicalize(url, false)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- canonicalize_url ---------------------------------------------------

    #[test]
    fn test_canonicalize_lowercase() {
        assert_eq!(
            canonicalize_url("HTTP://EXAMPLE.COM/Path"),
            "http://example.com/path"
        );
    }

    #[test]
    fn test_canonicalize_strips_tracking() {
        let url = "http://example.com/page?utm_source=test&fbclid=abc&_ga=123&keep=value";
        let result = canonicalize_url(url);
        assert!(!result.contains("utm_source"));
        assert!(!result.contains("fbclid"));
        assert!(!result.contains("_ga"));
        assert!(result.contains("keep=value"));
    }

    #[test]
    fn test_canonicalize_preserves_fragment() {
        let result = canonicalize_url("http://example.com/page#section1");
        assert!(result.contains("#section1"));
    }

    #[test]
    fn test_canonicalize_sorts_query_params() {
        assert_eq!(
            canonicalize_url("http://example.com/?z=1&a=2"),
            "http://example.com/?a=2&z=1"
        );
    }

    // -- dedup_key ----------------------------------------------------------

    #[test]
    fn test_dedup_key_strips_fragment() {
        let result = dedup_key("http://example.com/page#section1");
        assert!(!result.contains("#section1"));
        assert_eq!(result, "http://example.com/page");
    }

    #[test]
    fn test_dedup_key_strips_google_fragment() {
        let result = dedup_key("http://example.com/page#:~:text=hello");
        assert!(!result.contains("#:~:text="));
        assert_eq!(result, "http://example.com/page");
    }

    #[test]
    fn test_dedup_key_tracking_params_gone() {
        let url = "http://example.com/page?utm_source=test&mc_cid=123&a=1";
        let result = dedup_key(url);
        assert!(!result.contains("utm_source"));
        assert!(!result.contains("mc_cid"));
        assert!(result.contains("a=1"));
    }

    // -- edge cases ---------------------------------------------------------

    #[test]
    fn test_invalid_url_returns_original() {
        assert_eq!(canonicalize_url("not a url"), "not a url");
        assert_eq!(canonicalize_url(""), "");
    }

    #[test]
    fn test_canonicalize_double_slash_in_path() {
        assert_eq!(
            canonicalize_url("http://example.com//path//to//file"),
            "http://example.com/path/to/file"
        );
    }

    #[test]
    fn test_canonicalize_trailing_slash() {
        assert_eq!(
            canonicalize_url("http://example.com/page/"),
            "http://example.com/page"
        );
        // Root slash is preserved
        assert_eq!(
            canonicalize_url("http://example.com/"),
            "http://example.com/"
        );
    }
}
