/// Parse a URL string, returning `None` for unparseable inputs.
///
/// Shared parse helper used by [`extract_host`] and the url_normalizer
/// module so all URL parsing funnels through one place.
pub(crate) fn parse_url(url: &str) -> Option<url::Url> {
    url::Url::parse(url).ok()
}

/// Extract the host portion of a URL string.
///
/// Returns `None` when the URL cannot be parsed or has no host (e.g.
/// `mailto:` links).
pub fn extract_host(url: &str) -> Option<String> {
    parse_url(url)?.host_str().map(String::from)
}

#[cfg(test)]
mod tests {
    use super::extract_host;

    #[test]
    fn extract_host_from_http_url() {
        assert_eq!(
            extract_host("https://www.example.com/path?q=1"),
            Some("www.example.com".to_string())
        );
    }

    #[test]
    fn extract_host_missing_for_mailto() {
        assert_eq!(extract_host("mailto:user@example.com"), None);
    }
}
