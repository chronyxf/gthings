//! Post-processing for the Google backend: CAPTCHA/access-denied page
//! detection. Result cleaning (junk filter, base-URL dedup, title/snippet
//! cleanup, 1-based position renumbering) is engine-agnostic and lives in
//! the shared CDP module
//! ([`crate::engine::scrape::brave::shared::post_process_results`]); CDP
//! error mapping is the shared [`crate::engine::scrape::brave::shared::map_cdp_error`].

/// True when `url` looks like Google's CAPTCHA/Sorry block page.
pub(super) fn is_captcha_url(url: &str) -> bool {
    url.contains("/sorry/") || url.contains("google.com/sorry")
}

/// True when `title` looks like Google's access-denied ("Accessibility
/// help" / "Learn more") block page.
pub(super) fn is_captcha_title(title: &str) -> bool {
    title.contains("Accessibility") || title.contains("Learn more")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captcha_url_detection() {
        assert!(is_captcha_url(
            "https://www.google.com/sorry/index?continue=https://www.google.com/search?q=x"
        ));
        assert!(is_captcha_url(
            "https://google.com/sorry/?continue=https://www.google.com/"
        ));
        assert!(!is_captcha_url("https://www.google.com/search?q=rust"));
        assert!(!is_captcha_url("https://example.com/page"));
        assert!(!is_captcha_url(""));
    }

    #[test]
    fn captcha_title_detection() {
        assert!(is_captcha_title("Accessibility help"));
        assert!(is_captcha_title("Google - Learn more about this page"));
        assert!(!is_captcha_title("rust - Google Search"));
        assert!(!is_captcha_title(""));
    }
}
