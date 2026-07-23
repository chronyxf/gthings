/// Comprehensive error type for the gthings ecosystem.
///
/// Every fallible operation in the shared crate returns `Result<T, GthingsError>`.
#[derive(Debug, thiserror::Error)]
pub enum GthingsError {
    /// No browser process could be found listening on the given port.
    #[error("Browser not found on port {0}")]
    BrowserNotFound(u16),

    /// An error reported by the Chrome DevTools Protocol.
    #[error("CDP error: {code} ({message})")]
    CdpError {
        /// CDP error code.
        code: i64,
        /// Human-readable error message from the browser.
        message: String,
    },

    /// An error from the CDP transport layer (launch, connect, eval, etc.).
    #[error("CDP: {0}")]
    Cdp(String),

    /// An HTTP-level failure (e.g. connection refused, TLS error).
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    /// A local cache operation failed.
    #[error("Cache error: {0}")]
    Cache(String),

    /// Retrieved content did not meet minimum quality thresholds.
    #[error("Content quality check failed: {0}")]
    LowQuality(String),

    /// Wraps [`std::io::Error`].
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// An operation exceeded its allotted time budget.
    #[error("Timeout after {0}ms")]
    Timeout(u64),

    /// A string or data structure could not be parsed.
    #[error("Parse error: {0}")]
    Parse(String),

    /// A generic, unstructured error.
    #[error("{0}")]
    Other(String),
}

impl From<String> for GthingsError {
    fn from(msg: String) -> Self {
        GthingsError::Other(msg)
    }
}

impl From<&str> for GthingsError {
    fn from(msg: &str) -> Self {
        GthingsError::Other(msg.to_string())
    }
}
