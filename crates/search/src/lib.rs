//! Search crate — web search, page following, and batch operations via CDP.
//!
//! # Modules
//!
//! | Module | Description |
//! | [`search`] | Google search via CDP with attribute-based JS extraction |
//! | [`follow`] | Page content extraction via CDP JS evaluation |
//! | [`batch`] | Concurrent multi-query search |
//!
//! All modules delegate browser lifecycle and navigation to the caller-provided
//! [`Session`](gthings_cdp::Session) and [`Tab`](gthings_cdp::Tab).

pub mod batch;
pub mod follow;
pub mod harvest;
pub mod search;

pub use batch::{BatchProcessor, BatchSearchConfig};
pub use follow::{FollowResult, follow};
pub use gthings_common::domain_reputation::DomainReputation;
pub use harvest::{
    BatchHarvestRequest, BodyStatus, DedupStrategy, HarvestRunSummary, HarvestWarning,
    HarvestedResult, QueryCoverage, RankStrategy, harvest,
};
pub use search::{SearchResult, search};
