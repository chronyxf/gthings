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

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("WebSocket error: {0}")]
    Ws(#[from] tokio_tungstenite::tungstenite::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
}

pub type Result<T> = std::result::Result<T, CdpError>;

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
}
