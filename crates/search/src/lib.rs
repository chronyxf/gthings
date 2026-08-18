//! Search crate — web search, page following, and batch operations via CDP.
//!
//! # Modules
//!
//! | Module | Description |
//! | [`search`] | Multi-engine search facade (Google → Brave → Bing) |
//! | [`stream`] | Search event stream types ([`SearchEvent`]) |
//! | [`streaming`] | Streaming search facade ([`search_streaming`]) |
//! | [`follow`] | Page content extraction via CDP JS evaluation |
//! | [`batch`] | Concurrent multi-query search |
//! | [`engine`] | Multi-engine abstraction: engines, backends, shared HTTP |
//!
//! All modules delegate browser lifecycle and navigation to the caller-provided
//! [`Session`](gthings_cdp::Session) and [`Tab`](gthings_cdp::Tab).
//!
//! The streaming facade ([`search_streaming`]) is the single search path: it
//! emits progressive [`SearchEvent`]s over an mpsc channel, and the collect
//! facades ([`search`], [`search_with_engine`]) are projections that consume
//! the same event stream.

pub mod batch;
pub mod engine;
pub mod follow;
pub mod harvest;
pub mod search;
pub mod stream;
pub mod streaming;

pub use batch::{BatchProcessor, BatchSearchConfig};
pub use engine::api::brave::BraveApiBackend;
pub use engine::api::tavily::TavilyBackend;
pub use engine::pacing::{PacingSnapshot, PacingStore};
pub use engine::{
    EngineChoice, EngineMode, EngineSearchResult, SearchEngine, SearchEngineBackend,
    SearchEngineError, technique,
};
pub use follow::{FollowResult, follow};
pub use gthings_common::domain_reputation::DomainReputation;
pub use harvest::{
    BatchHarvestRequest, BodyStatus, HarvestRunSummary, HarvestWarning, HarvestedResult,
    QueryCoverage, RankStrategy, harvest,
};
pub use search::{SearchResult, search, search_with_engine};
pub use stream::{EngineEventKind, SearchEvent, Sender};
pub use streaming::search_streaming;
