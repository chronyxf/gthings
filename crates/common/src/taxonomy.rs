//! Canonical error-code taxonomy shared by the CLI and the serve daemon.
//!
//! This is the **only** source of truth for the nine error codes that can
//! appear in an [`crate::envelope::ErrorBody`] `code` field. Per-crate mapping
//! (internal error → [`ErrorCode`]) lives in the `gthings-cli` crate; this
//! leaf crate intentionally knows nothing about other crates' errors.

use std::fmt;

use serde::de::{self, Deserializer};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

/// The canonical, wire-stable error taxonomy.
///
/// The serialized form is kebab-case and matches [`Display`](fmt::Display), so
/// `"rate-limited"` is what Go and other consumers receive regardless of
/// whether they deserialize the envelope or match on the string. Both serde
/// and [`Display`](fmt::Display) share the single mapping in [`ErrorCode::as_str`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    /// The operation exceeded its allotted time budget.
    Timeout,
    /// The upstream engine or daemon throttled the request.
    RateLimited,
    /// An aggregate/account quota has been exhausted.
    QuotaExceeded,
    /// The target site presented a CAPTCHA / bot wall.
    Captcha,
    /// The CDP browser could not be located or launched.
    BrowserNotFound,
    /// The CDP connection could not be established.
    ConnectionFailed,
    /// The request payload was malformed or failed validation.
    InvalidInput,
    /// The search/engine pipeline failed.
    EngineFailed,
    /// Content extraction failed.
    ExtractFailed,
}

impl ErrorCode {
    /// Return the wire-format (kebab-case) string for this code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::RateLimited => "rate-limited",
            Self::QuotaExceeded => "quota-exceeded",
            Self::Captcha => "captcha",
            Self::BrowserNotFound => "browser-not-found",
            Self::ConnectionFailed => "connection-failed",
            Self::InvalidInput => "invalid-input",
            Self::EngineFailed => "engine-failed",
            Self::ExtractFailed => "extract-failed",
        }
    }

    /// Parse a wire-format (kebab-case) string back into a code.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        <Self as std::str::FromStr>::from_str(s).ok()
    }
}

impl std::str::FromStr for ErrorCode {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "timeout" => Self::Timeout,
            "rate-limited" => Self::RateLimited,
            "quota-exceeded" => Self::QuotaExceeded,
            "captcha" => Self::Captcha,
            "browser-not-found" => Self::BrowserNotFound,
            "connection-failed" => Self::ConnectionFailed,
            "invalid-input" => Self::InvalidInput,
            "engine-failed" => Self::EngineFailed,
            "extract-failed" => Self::ExtractFailed,
            _ => return Err(()),
        })
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for ErrorCode {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ErrorCode {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::parse(&s).ok_or_else(|| de::Error::custom(format!("unknown error code: {s}")))
    }
}

#[cfg(test)]
mod tests {
    use super::ErrorCode;

    #[test]
    fn display_is_lowercase_kebab() {
        assert_eq!(ErrorCode::Timeout.to_string(), "timeout");
        assert_eq!(ErrorCode::RateLimited.to_string(), "rate-limited");
        assert_eq!(ErrorCode::QuotaExceeded.to_string(), "quota-exceeded");
        assert_eq!(ErrorCode::Captcha.to_string(), "captcha");
        assert_eq!(ErrorCode::BrowserNotFound.to_string(), "browser-not-found");
        assert_eq!(ErrorCode::ConnectionFailed.to_string(), "connection-failed");
        assert_eq!(ErrorCode::InvalidInput.to_string(), "invalid-input");
        assert_eq!(ErrorCode::EngineFailed.to_string(), "engine-failed");
        assert_eq!(ErrorCode::ExtractFailed.to_string(), "extract-failed");
    }

    #[test]
    fn serialize_is_kebab_case() {
        for code in [
            ErrorCode::Timeout,
            ErrorCode::RateLimited,
            ErrorCode::QuotaExceeded,
            ErrorCode::Captcha,
            ErrorCode::BrowserNotFound,
            ErrorCode::ConnectionFailed,
            ErrorCode::InvalidInput,
            ErrorCode::EngineFailed,
            ErrorCode::ExtractFailed,
        ] {
            assert_eq!(serde_json::to_string(&code).unwrap(), format!("\"{code}\""));
        }
    }
}
