//! Non-API search engine backends (scrape / HTML extraction).
//!
//! Concrete [`SearchEngineBackend`](crate::engine::SearchEngineBackend)
//! implementations that scrape result pages rather than calling a vendor
//! API: [`brave`](brave) (Brave SERP via CDP), [`bing`](bing) (Bing RSS),
//! and [`google`](google) (Google SERP via CDP).

pub mod bing;
pub mod brave;
pub mod google;
