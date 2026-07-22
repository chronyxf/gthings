//! Search crate — web search orchestration, page following, batch operations,
//! and harvest pipelines.
//!
//! This crate replaces the existing shell scripts in `skills/gsearch/lib/`.
//!
//! # Modules
//!
//! | Module | Description |
//! |--------|-------------|
//! | [`types`] | Shared data types for search, follow, and batch operations |
//! | [`search`] | Google search via browser daemon |
//! | [`follow`] | Page following and content extraction with caching & quality gates |
//! | [`batch`] | Batch search, follow, and two-phase harvest pipeline |
//!
//! All CDP operations are dispatched via UDS to the `browser-daemon`,
//! which communicates with Chrome through `cdp-core`. No TypeScript
//! subprocess is involved.

pub mod batch;
pub mod follow;
pub mod search;
pub mod types;

// Re-exports for convenience
pub use batch::BatchProcessor;
pub use follow::PageFollower;
pub use search::GoogleSearch;
pub use types::*;
