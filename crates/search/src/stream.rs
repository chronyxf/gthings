//! Search event stream types.
//!
//! The serve daemon's SSE projection consumes a [`SearchEvent`] stream emitted
//! by [`crate::search_streaming`]. The stream is the ONE internal search path:
//! the collect facades ([`crate::search::search_with_router`]) are projections
//! that consume the same events and drop the progress metadata.

use tokio::sync::mpsc;

use crate::SearchResult;
use crate::engine::{SearchEngine, SearchEngineError};

/// Tokio mpsc sender used to emit [`SearchEvent`]s.
pub type Sender = mpsc::Sender<SearchEvent>;

/// A progressive event emitted by [`crate::search_streaming`].
///
/// Every stream is terminal: it ends with exactly one [`SearchEvent::Done`] or
/// [`SearchEvent::Error`]. Backends are collect-all (they only answer once a
/// query is fully complete), so `Result` events materialize across
/// engines/queries rather than within a single engine response — they are
/// emitted post-map so position and dedup semantics are preserved.
#[derive(Debug)]
pub enum SearchEvent {
    /// The search job has started (the first event of every stream).
    JobStarted,
    /// A single mapped search result (post-map, position/dedup preserved).
    ///
    /// Boxed to keep the enum small (a [`SearchResult`] is large after adding
    /// score/published_date/favicon/mode fields).
    Result(Box<SearchResult>),
    /// An engine lifecycle event observed at the dispatch outcome funnel.
    EngineEvent {
        /// The engine the event concerns.
        engine: SearchEngine,
        /// What happened to that engine.
        kind: EngineEventKind,
    },
    /// The search completed successfully (final event of a successful stream).
    Done,
    /// The search failed terminally (final event of a failed stream).
    Error(SearchEngineError),
}

/// Classification of an [`SearchEvent::EngineEvent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum EngineEventKind {
    /// The engine served nothing and the router moved on to the next engine.
    Fallback,
    /// The engine was rate-limited (HTTP 429) and entered a cooldown.
    RateLimited,
    /// The engine served a captcha/block page and entered a cooldown.
    Captcha,
    /// The engine is now in a cooldown block (rate-limit/captcha).
    Cooldown,
}
