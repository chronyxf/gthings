// ---------------------------------------------------------------------------
// URL canonicalization and normalisation
// ---------------------------------------------------------------------------

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

/// Known multi-part TLDs that need special handling for registered-domain
/// extraction.
const MULTI_PART_TLDS: &[&str] = &[
    "co.uk", "org.uk", "ac.uk", "gov.uk", "com.au", "net.au", "org.au", "co.nz", "org.nz", "co.jp",
    "com.br", "co.kr", "co.in", "com.mx", "co.za", "com.ar", "com.cn",
];

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

fn is_tracking_param(key: &str) -> bool {
    TRACKING_PARAMS.contains(&key)
}

/// Build a percent-encoded query string from sorted key-value pairs.
fn build_query_string(pairs: &[(String, String)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| {
            let encoded_k: String = url::form_urlencoded::byte_serialize(k.as_bytes()).collect();
            if v.is_empty() {
                encoded_k
            } else {
                let encoded_v: String =
                    url::form_urlencoded::byte_serialize(v.as_bytes()).collect();
                format!("{}={}", encoded_k, encoded_v)
            }
        })
        .collect::<Vec<_>>()
        .join("&")
}

/// Normalise a URL path:
/// - lowercase
/// - collapse `//` → `/`
/// - strip trailing `/` (except root `/`)
fn normalize_path(path: &str) -> String {
    let lower = path.to_lowercase();
    let segments: Vec<&str> = lower.split('/').filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", segments.join("/"))
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Strip Google text fragment identifiers (`#:~:text=...`) from a URL.
pub fn strip_google_fragment(url: &str) -> String {
    if let Some(pos) = url.find("#:~:text=") {
        url[..pos].to_string()
    } else {
        url.to_string()
    }
}

/// Strip all URL fragments (`#` and everything after).
pub fn strip_all_fragments(url: &str) -> String {
    if let Some(pos) = url.find('#') {
        url[..pos].to_string()
    } else {
        url.to_string()
    }
}

/// Strip common tracking query parameters from a URL.
///
/// This does **not** perform any other canonicalization (no lowercasing,
/// no path normalisation, no fragment removal).
pub fn strip_tracking_params(url: &str) -> String {
    let mut parsed = match Url::parse(url) {
        Ok(u) => u,
        Err(_) => return url.to_string(),
    };

    let pairs: Vec<(String, String)> = parsed
        .query_pairs()
        .filter(|(k, _)| !is_tracking_param(k))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();

    let query_str = if pairs.is_empty() {
        None
    } else {
        Some(build_query_string(&pairs))
    };
    parsed.set_query(query_str.as_deref());

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
pub fn canonicalize_url(url: &str) -> String {
    let mut parsed = match Url::parse(url) {
        Ok(u) => u,
        Err(_) => return url.to_string(),
    };

    // Lowercase host
    if let Some(host) = parsed.host_str() {
        let lower_host = host.to_lowercase();
        // Note: set_host always succeeds because lower_host comes from the parsed URL's own host_str
        let _ = parsed.set_host(Some(&lower_host));
    }

    // Normalise path
    let normalized_path = normalize_path(parsed.path());
    parsed.set_path(&normalized_path);

    // Strip tracking params and sort remaining query params
    let pairs: Vec<(String, String)> = parsed
        .query_pairs()
        .filter(|(k, _)| !is_tracking_param(k))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();

    if pairs.is_empty() {
        parsed.set_query(None);
    } else {
        let mut sorted = pairs;
        sorted.sort_by(|a, b| a.0.cmp(&b.0));
        let query_str = build_query_string(&sorted);
        parsed.set_query(Some(&query_str));
    }

    // Fragment is preserved for display
    parsed.as_str().to_string()
}

/// Generate a deterministic dedup key (for comparison / dedup purposes).
///
/// Like [`canonicalize_url`] but:
/// - Fragment is **always** stripped
/// - Google `#:~:text=...` fragments are stripped first
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
pub fn is_pdf_url(url: &str) -> bool {
    let parsed = match Url::parse(url) {
        Ok(u) => u,
        Err(_) => return false,
    };

    if parsed.path().to_lowercase().ends_with(".pdf") {
        return true;
    }

    if is_arxiv_host(parsed.host_str()) && parsed.path().starts_with("/pdf/") {
        return true;
    }

    false
}

/// Check if a URL looks like an arXiv abstract or PDF page.
pub fn is_arxiv_url(url: &str) -> bool {
    let parsed = match Url::parse(url) {
        Ok(u) => u,
        Err(_) => return false,
    };

    if is_arxiv_host(parsed.host_str()) {
        let path = parsed.path();
        return path.starts_with("/abs/") || path.starts_with("/pdf/");
    }

    false
}

/// Extract the registered domain (e.g. `"example.com"` from
/// `"sub.example.com"`, or `"example.co.uk"` from `"sub.example.co.uk"`).
pub fn registered_domain(url: &str) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    let host = parsed.host_str()?;
    let parts: Vec<&str> = host.split('.').collect();

    if parts.len() < 2 {
        return None;
    }

    // Check whether the final two parts form a known multi-part TLD.
    let take_three = if parts.len() >= 3 {
        let candidate = format!("{}.{}", parts[parts.len() - 2], parts[parts.len() - 1]);
        MULTI_PART_TLDS.contains(&candidate.as_str())
    } else {
        false
    };

    if take_three {
        Some(parts[parts.len() - 3..].join("."))
    } else {
        Some(parts[parts.len() - 2..].join("."))
    }
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
