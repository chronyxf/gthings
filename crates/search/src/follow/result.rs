//! Error-result and provenance construction shared by follow paths.
//!
//! Extracted so both the [`follow`](crate::follow::follow) early-exit paths
//! and `harvest/orchestrator/follow.rs` construct error results identically
//! instead of duplicating the field list.

use chrono::Utc;
use gthings_common::provenance::{ExtractionMethod, Provenance};

use crate::follow::FollowResult;

/// Build a boilerplate [`FollowResult`] for early-exit error paths.
pub(crate) fn make_error_result(url: &str, error: &str, duration_ms: u64) -> FollowResult {
    FollowResult {
        url: url.to_string(),
        title: String::new(),
        content: String::new(),
        error: error.to_string(),
        provenance: error_provenance(url, duration_ms),
        pagination: None,
        quality_flags: Vec::new(),
    }
}

/// Build the [`Provenance`] shared by all error-result paths.
///
/// Extracted so both `follow.rs` and `harvest/orchestrator/follow.rs`
/// construct the error provenance identically instead of duplicating the
/// field list.
pub(crate) fn error_provenance(url: &str, duration_ms: u64) -> Provenance {
    Provenance {
        source_url: url.to_string(),
        method: ExtractionMethod::Follow,
        agent: gthings_common::user_agent::gthings_agent(),
        accessed_at: Utc::now(),
        duration_ms,
    }
}
