use thiserror::Error;

#[derive(Error, Debug)]
pub enum CdpError {
    #[error("Browser launch failed: {0}")]
    LaunchFailed(String),

    #[error("WebSocket error: {0}")]
    Ws(#[from] tokio_tungstenite::tungstenite::Error),

    #[error("CDP command failed: {msg} (method: {method})")]
    CommandFailed { method: String, msg: String },

    #[error("Timeout after {0}ms")]
    Timeout(u64),

    #[error("Oneshot channel broken")]
    ChannelBroken,

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Chrome process exited unexpectedly with code {0}")]
    ChromeExited(i32),

    #[error("Could not find DevTools WebSocket URL in Chrome output")]
    NoWsUrl,
}

pub type Result<T> = std::result::Result<T, CdpError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = CdpError::LaunchFailed("test".into());
        assert_eq!(format!("{}", err), "Browser launch failed: test");

        let err = CdpError::NoWsUrl;
        assert_eq!(
            format!("{}", err),
            "Could not find DevTools WebSocket URL in Chrome output"
        );

        let err = CdpError::Timeout(5000);
        assert_eq!(format!("{}", err), "Timeout after 5000ms");

        let err = CdpError::CommandFailed {
            method: "Page.navigate".into(),
            msg: "timeout".into(),
        };
        assert_eq!(
            format!("{}", err),
            "CDP command failed: timeout (method: Page.navigate)"
        );
    }

    #[test]
    fn test_error_debug() {
        let err = CdpError::NoWsUrl;
        assert!(format!("{:?}", err).contains("NoWsUrl"));
    }
}
