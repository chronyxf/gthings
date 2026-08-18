// ---------------------------------------------------------------------------
// URL classification: PDF / arXiv / registered domain
// ---------------------------------------------------------------------------

use super::try_parse_url;

fn is_arxiv_host(host: Option<&str>) -> bool {
    matches!(host, Some(h) if h == "arxiv.org" || h.ends_with(".arxiv.org"))
}

/// Whether a path is an arXiv abstract or PDF page.
fn is_arxiv_path(path: &str) -> bool {
    path.starts_with("/abs/") || path.starts_with("/pdf/")
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Check if a URL appears to point to a PDF.
///
/// Returns `true` when:
/// - The path ends with `.pdf` (case-insensitive), **or**
/// - It is an `arxiv.org/pdf/...` URL.
#[must_use]
pub fn is_pdf_url(url: &str) -> bool {
    let Some(parsed) = try_parse_url(url) else {
        return false;
    };

    if parsed.path().to_ascii_lowercase().ends_with(".pdf") {
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

    is_arxiv_host(parsed.host_str()) && is_arxiv_path(parsed.path())
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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

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
}
