// ---------------------------------------------------------------------------
// URL canonicalization and normalisation
// ---------------------------------------------------------------------------

use std::borrow::Cow;

use url::Url;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Query-parameter keys that are considered tracking / marketing noise.
const TRACKING_PARAMS: &[&str] = &[
    "utm_source",
    "utm_medium",
    "utm_campaign",
    "utm_term",
    "utm_content",
    "fbclid",
    "gclid",
    "_ga",
    "_gl",
    "mc_cid",
    "mc_eid",
];

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

fn is_tracking_param(key: &str) -> bool {
    TRACKING_PARAMS.contains(&key)
}

/// Build a percent-encoded query string from key-value pairs.
///
/// Avoids allocating an intermediate `Vec<String>` — writes directly into
/// a single `String` buffer.
fn build_query_string<K: AsRef<str>, V: AsRef<str>>(pairs: &[(K, V)]) -> String {
    let mut result = String::new();
    for (i, (k, v)) in pairs.iter().enumerate() {
        if i > 0 {
            result.push('&');
        }
        result.extend(url::form_urlencoded::byte_serialize(k.as_ref().as_bytes()));
        if !v.as_ref().is_empty() {
            result.push('=');
            result.extend(url::form_urlencoded::byte_serialize(v.as_ref().as_bytes()));
        }
    }
    result
}

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

/// Filter tracking query parameters and build a query string.
///
/// Returns the percent-encoded query string, or an empty string when no
/// non-tracking parameters remain.
fn filter_and_build_query(url: &Url) -> String {
    let pairs: Vec<(Cow<'_, str>, Cow<'_, str>)> = url
        .query_pairs()
        .filter(|(k, _)| !is_tracking_param(k))
        .collect();
    build_query_string(&pairs)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Parse a URL string, returning `None` for unparseable inputs.
fn try_parse_url(url: &str) -> Option<Url> {
    Url::parse(url).ok()
}

/// Strip a suffix pattern from a URL string, respecting UTF-8 boundaries.
#[allow(clippy::incompatible_msrv)]
fn strip_suffix(url: &str, pattern: &str) -> String {
    if let Some(pos) = url.find(pattern) {
        url[..url.floor_char_boundary(pos)].to_string()
    } else {
        url.to_string()
    }
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

/// Strip Google text fragment identifiers (`#:~:text=...`) from a URL.
#[must_use]
pub fn strip_google_fragment(url: &str) -> String {
    strip_suffix(url, "#:~:text=")
}

/// Strip all URL fragments (`#` and everything after).
#[must_use]
pub fn strip_all_fragments(url: &str) -> String {
    strip_suffix(url, "#")
}

/// Strip common tracking query parameters from a URL.
///
/// This does **not** perform any other canonicalization (no lowercasing,
/// no path normalisation, no fragment removal).
#[must_use]
pub fn strip_tracking_params(url: &str) -> String {
    let Some(mut parsed) = try_parse_url(url) else {
        return url.to_string();
    };

    let query_str = filter_and_build_query(&parsed);
    if query_str.is_empty() {
        parsed.set_query(None);
    } else {
        parsed.set_query(Some(&query_str));
    }

    parsed.as_str().to_string()
}

/// Fully canonicalize a URL for human display:
///
/// - Lowercase scheme + host
/// - Strip tracking parameters
/// - Lowercase path
/// - Strip trailing slash (except for root `/`)
/// - Collapse `//` → `/` in path
/// - Preserve fragment (for display)
/// - Sort remaining query parameters alphabetically
#[must_use]
pub fn canonicalize_url(url: &str) -> String {
    let Some(mut parsed) = try_parse_url(url) else {
        return url.to_string();
    };

    canonicalize_parsed_url(&mut parsed);

    // Fragment is preserved for display
    parsed.as_str().to_string()
}

/// Generate a deterministic dedup key (for comparison / dedup purposes).
///
/// Like [`canonicalize_url`] but:
/// - Fragment is **always** stripped
/// - Google `#:~:text=...` fragments are stripped first
#[must_use]
pub fn dedup_key(url: &str) -> String {
    let url = strip_google_fragment(url);
    let url = strip_all_fragments(&url);
    canonicalize_url(&url)
}

/// Check if a URL appears to point to a PDF.
///
/// Returns `true` when:
/// - The path ends with `.pdf` (case-insensitive), **or**
/// - It is an `arxiv.org/pdf/...` URL.
#[must_use]
#[allow(clippy::incompatible_msrv)]
pub fn is_pdf_url(url: &str) -> bool {
    let Some(parsed) = try_parse_url(url) else {
        return false;
    };

    let pdf_path = parsed.path();
    if pdf_path.len() >= 4
        && pdf_path[pdf_path.floor_char_boundary(pdf_path.len() - 4)..].eq_ignore_ascii_case(".pdf")
    {
        return true;
    }

    if is_arxiv_host(parsed.host_str()) && parsed.path().starts_with("/pdf/") {
        return true;
    }

    false
}

/// Check if a URL looks like an arXiv abstract or PDF page.
#[must_use]
pub fn is_arxiv_url(url: &str) -> bool {
    let Some(parsed) = try_parse_url(url) else {
        return false;
    };

    if is_arxiv_host(parsed.host_str()) {
        let path = parsed.path();
        return path.starts_with("/abs/") || path.starts_with("/pdf/");
    }

    false
}

/// Extract the registered domain (e.g. `"example.com"` from
/// `"sub.example.com"`, or `"example.co.uk"` from `"sub.example.co.uk"`).
///
/// Uses the [Public Suffix List](https://publicsuffix.org/) via the `psl` crate,
/// replacing a previously hardcoded and incomplete multi-part TLD table.
///
/// Returns `None` when the URL cannot be parsed or has no host.
#[must_use]
pub fn registered_domain(url: &str) -> Option<String> {
    let parsed = try_parse_url(url)?;
    let host = parsed.host_str()?;
    psl::domain_str(host).map(ToString::to_string)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn is_arxiv_host(host: Option<&str>) -> bool {
    matches!(host, Some(h) if h == "arxiv.org" || h.ends_with(".arxiv.org"))
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

    // -- strip_google_fragment ------------------------------------------------

    #[test]
    fn test_strip_google_fragment() {
        assert_eq!(
            strip_google_fragment("http://example.com#:~:text=match"),
            "http://example.com"
        );
        // Regular fragments are left alone
        assert_eq!(
            strip_google_fragment("http://example.com#section"),
            "http://example.com#section"
        );
    }

    // -- is_pdf_url ---------------------------------------------------------

    #[test]
    fn test_is_pdf_url() {
        assert!(is_pdf_url("http://example.com/doc.pdf"));
        assert!(is_pdf_url("https://arxiv.org/pdf/2301.00001"));
        assert!(!is_pdf_url("http://example.com/doc.txt"));
        assert!(!is_pdf_url("not a url"));
    }

    // -- is_arxiv_url -------------------------------------------------------

    #[test]
    fn test_is_arxiv_url() {
        assert!(is_arxiv_url("https://arxiv.org/abs/2301.00001"));
        assert!(is_arxiv_url("https://arxiv.org/pdf/2301.00001"));
        assert!(!is_arxiv_url("http://example.com"));
        assert!(!is_arxiv_url("not a url"));
    }

    // -- registered_domain --------------------------------------------------

    #[test]
    fn test_registered_domain() {
        assert_eq!(
            registered_domain("http://sub.example.co.uk/page"),
            Some("example.co.uk".to_string())
        );
        assert_eq!(
            registered_domain("http://sub.example.com/page"),
            Some("example.com".to_string())
        );
        assert_eq!(
            registered_domain("http://example.co.uk/page"),
            Some("example.co.uk".to_string())
        );
        assert_eq!(registered_domain("not a url"), None);
    }

    // -- edge cases ---------------------------------------------------------

    #[test]
    fn test_invalid_url_returns_original() {
        assert_eq!(canonicalize_url("not a url"), "not a url");
        assert_eq!(canonicalize_url(""), "");
    }

    #[test]
    fn test_strip_all_fragments() {
        assert_eq!(
            strip_all_fragments("http://example.com/page#section"),
            "http://example.com/page"
        );
        assert_eq!(
            strip_all_fragments("http://example.com/page"),
            "http://example.com/page"
        );
    }

    #[test]
    fn test_strip_tracking_params() {
        let url = "http://example.com/page?utm_source=test&keep=value&gclid=xyz";
        let result = strip_tracking_params(url);
        assert!(!result.contains("utm_source"));
        assert!(!result.contains("gclid"));
        assert!(result.contains("keep=value"));
        // Should preserve the scheme + host case as-is (no other normalisation)
        assert!(result.starts_with("http://example.com/page?keep=value"));
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
