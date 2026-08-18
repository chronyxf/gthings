// ---------------------------------------------------------------------------
// Fragment stripping
// ---------------------------------------------------------------------------

/// Strip all URL fragments (`#` and everything after), respecting UTF-8
/// boundaries.
///
/// Only used by tests — canonicalization strips fragments via the parsed
/// [`url::Url`] API instead.
#[cfg(test)]
pub(crate) fn strip_all_fragments(url: &str) -> String {
    match url.find('#') {
        Some(pos) => url[..pos].to_string(),
        None => url.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::strip_all_fragments;

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
}
