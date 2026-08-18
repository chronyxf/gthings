//! Small, dependency-light helper functions shared across the workspace.
//!
//! Each submodule owns one concern:
//! - [`fs`] — filesystem helpers (mtime expiry checks, atomic writes)
//! - [`url`] — URL string helpers (host extraction)
//! - [`str`] — string helpers (boundary-safe suffix truncation)
//! - [`time`] — time helpers (Unix millisecond clock)

pub mod fs;
pub mod str;
pub mod time;
pub mod url;
