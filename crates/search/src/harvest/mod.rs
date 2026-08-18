//! Full research pipeline: search → dedup → rank → follow.
//!
//! Orchestrates multiple search queries, deduplicates results by normalized URL,
//! applies a configurable ranking strategy, and follows the top-N results for
//! full content extraction with quality scoring.
//!
//! This module is split into sub-modules:
//! - [`types`] — Data types and enums
//! - [`orchestrator`] — Top-level orchestration (harvest, phase_search, phase_follow)
//! - [`ranking`] — Dedup and ranking logic
//! - [`quality`] — Quality scoring (directory module; `quality::sections` extracts sections)

pub(crate) mod orchestrator;
mod quality;
mod ranking;
pub(crate) mod types;

// Re-export public API — must match previous public surface exactly
pub use orchestrator::harvest;
pub(crate) use orchestrator::is_junk_url;
pub use types::{
    BatchHarvestRequest, BodyStatus, HarvestRunSummary, HarvestWarning, HarvestedResult,
    QueryCoverage, RankStrategy,
};
