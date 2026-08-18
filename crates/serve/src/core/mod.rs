//! In-process machinery backing the serve daemon.
//!
//! - [`queue`] — bounded async job queue with concurrency control.
//! - [`workers`] — job execution and SSE registry.
//! - [`shutdown`] — drain-on-SIGTERM/SIGINT graceful teardown.

pub(crate) mod queue;
pub(crate) mod shutdown;
pub(crate) mod workers;
