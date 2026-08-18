//! Post-processing for the Brave backend: CAPTCHA/verification page
//! detection. Result cleaning (junk filter, base-URL dedup, title/snippet
//! cleanup, 1-based position renumbering) is engine-agnostic and lives in
//! the shared CDP module
//! ([`crate::engine::scrape::brave::shared::post_process_results`]); CDP
//! error mapping is the shared [`crate::engine::scrape::brave::shared::map_cdp_error`].

/// True when `url` looks like Brave's bot-verification/CAPTCHA block page.
/// Brave redirects challenged clients to `search.brave.com/verify` (a
/// "verify you are human" page); the host is pinned so an external
/// result's `/verify` path can never false-positive.
pub(super) fn is_captcha_url(url: &str) -> bool {
    url.contains("search.brave.com/verify") || url.contains("search.brave.com/captcha")
}

/// True when `title` looks like Brave's verification page or a Cloudflare
/// interstitial ("Just a moment..." challenge) served in front of the SERP.
pub(super) fn is_captcha_title(title: &str) -> bool {
    title.contains("Verify you are human") || title.contains("Just a moment")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captcha_url_detection() {
        assert!(is_captcha_url(
            "https://search.brave.com/verify?source=web&q=rust"
        ));
        assert!(is_captcha_url("https://search.brave.com/captcha"));
        assert!(!is_captcha_url("https://search.brave.com/search?q=rust"));
        assert!(
            !is_captcha_url("https://example.com/verify"),
            "external /verify path must not false-positive"
        );
        assert!(!is_captcha_url(""));
    }

    #[test]
    fn captcha_title_detection() {
        assert!(is_captcha_title("Verify you are human"));
        assert!(is_captcha_title("Just a moment..."));
        assert!(!is_captcha_title("rust - Brave Search"));
        assert!(!is_captcha_title(""));
    }
}
