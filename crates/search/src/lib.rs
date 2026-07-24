//! Search crate — web search orchestration, page following, batch operations,
//! and harvest pipelines.
//!
//! # Modules
//!
//! | Module | Description |
//! | [`types`] | Shared data types for search, follow, and batch operations |
//! | [`search`] | Google search via ephemeral Chrome CDP sessions |
//! | [`follow`] | Page following and content extraction with caching & quality gates |
//! | [`batch`] | Batch search, follow, and two-phase harvest pipeline |
//!
//! Browser operations launch an ephemeral Chrome instance per operation (or
//! per batch), communicate via the Chrome DevTools Protocol (CDP) WebSocket,
//! and shut Chrome down on Drop.

pub mod batch;
pub mod follow;
pub mod search;
pub mod types;

pub use batch::BatchProcessor;
pub use follow::PageFollower;
pub use search::GoogleSearch;
pub use types::{
    BatchSearchResult, FollowOpts, FollowResult, HarvestMeta, HarvestResult, SearchMeta,
    SearchResult,
};
