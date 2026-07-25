/// Comprehensive error type for the gthings ecosystem.
///
/// Every fallible operation in the shared crate returns `Result<T, GthingsError>`.
#[derive(Debug, thiserror::Error)]
pub enum GthingsError {
    /// Wraps [`std::io::Error`].
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

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
