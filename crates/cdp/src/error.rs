use thiserror::Error;

#[derive(Debug, Error)]
pub enum CdpError {
    #[error("Browser not found on port {port}")]
    BrowserNotFound { port: u16 },

    #[error("Connection failed: {detail}")]
    ConnectionFailed { detail: String },

    #[error("CDP call {method} failed: {detail}")]
    CdpCallFailed { method: String, detail: String },

    #[error("Navigation timeout: {url} did not load within {timeout}s")]
    NavigationTimeout { url: String, timeout: u64 },

    #[error(
        "Unsupported URL scheme `{scheme}` in `{url}` (only http/https/about:blank are allowed)"
    )]
    UnsupportedUrl { scheme: String, url: String },

    #[error("Google CAPTCHA/Sorry block: {detail}")]
    CaptchaBlocked { detail: String },

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("WebSocket error: {0}")]
    Ws(#[from] Box<tokio_tungstenite::tungstenite::Error>),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
}

pub type Result<T> = std::result::Result<T, CdpError>;

/// Validate that a URL is safe to pass to Chrome's navigation primitives
/// (`Page.navigate`, `Target.createTarget`).
///
/// Only `http:`, `https:`, and the exact `about:blank` constant are allowed.
/// All other schemes (`file:`, `data:`, `javascript:`, `chrome:`, `blob:`,
/// `ftp:`, and any `about:` page other than `about:blank`) are rejected to
/// prevent SSRF via the browser.
pub(crate) fn validate_scheme(url: &str) -> Result<()> {
    if url == crate::ABOUT_BLANK {
        return Ok(());
    }
    let scheme = url
        .split_once("://")
        .map(|(s, _)| s)
        .or_else(|| url.split_once(':').map(|(s, _)| s))
        .unwrap_or("");
    match scheme {
        "http" | "https" => Ok(()),
        _ => Err(CdpError::UnsupportedUrl {
            scheme: scheme.to_string(),
            url: url.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display_browser_not_found() {
        let err = CdpError::BrowserNotFound { port: 9222 };
        assert_eq!(format!("{}", err), "Browser not found on port 9222");
    }

    #[test]
    fn test_error_display_connection_failed() {
        let err = CdpError::ConnectionFailed {
            detail: "refused".into(),
        };
        assert_eq!(format!("{}", err), "Connection failed: refused");
    }

    #[test]
    fn test_error_display_cdp_call_failed() {
        let err = CdpError::CdpCallFailed {
            method: "Page.navigate".into(),
            detail: "timeout".into(),
        };
        assert_eq!(format!("{}", err), "CDP call Page.navigate failed: timeout");
    }

    #[test]
    fn test_error_display_navigation_timeout() {
        let err = CdpError::NavigationTimeout {
            url: "https://example.com".into(),
            timeout: 30,
        };
        assert_eq!(
            format!("{}", err),
            "Navigation timeout: https://example.com did not load within 30s"
        );
    }

    #[test]
    fn test_error_display_captcha_blocked() {
        let err = CdpError::CaptchaBlocked {
            detail: "Google served a CAPTCHA challenge page".into(),
        };
        assert_eq!(
            format!("{}", err),
            "Google CAPTCHA/Sorry block: Google served a CAPTCHA challenge page"
        );
    }

    #[test]
    fn test_error_display_unsupported_url() {
        let err = CdpError::UnsupportedUrl {
            scheme: "file".into(),
            url: "file:///etc/passwd".into(),
        };
        assert_eq!(
            format!("{}", err),
            "Unsupported URL scheme `file` in `file:///etc/passwd` (only http/https/about:blank are allowed)"
        );
    }

    #[test]
    fn test_validate_scheme_allows_http() {
        validate_scheme("http://example.com").unwrap();
    }

    #[test]
    fn test_validate_scheme_allows_https() {
        validate_scheme("https://example.com/path?q=1").unwrap();
    }

    #[test]
    fn test_validate_scheme_allows_about_blank() {
        validate_scheme(crate::ABOUT_BLANK).unwrap();
    }

    #[test]
    fn test_validate_scheme_rejects_unsafe_schemes() {
        let bad_urls = [
            "file:///etc/passwd",
            "data:text/html,<script>alert(1)</script>",
            "javascript:alert(1)",
            "chrome://flags",
            "blob:https://example.com/uuid",
            "ftp://example.com/file",
            "about:config",
        ];
        for url in bad_urls {
            match validate_scheme(url) {
                Err(CdpError::UnsupportedUrl { scheme, url: got }) => {
                    assert_eq!(got, url);
                    assert!(!scheme.is_empty(), "scheme missing for {url}");
                }
                other => panic!("expected UnsupportedUrl for {url}, got {other:?}"),
            }
        }
    }

    #[test]
    fn test_validate_scheme_rejects_scheme_less_url() {
        assert!(matches!(
            validate_scheme("example.com/no-scheme"),
            Err(CdpError::UnsupportedUrl { .. })
        ));
    }
}
