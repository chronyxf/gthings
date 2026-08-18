//! API-based search engine backends (paid, plain HTTP, JSON).
//!
//! Concrete [`SearchEngineBackend`](crate::engine::SearchEngineBackend)
//! implementations that call a vendor search API over plain HTTP and parse
//! JSON responses: [`brave`](brave) (Brave Search API) and
//! [`tavily`](tavily) (Tavily Search API).

pub mod brave;
pub mod tavily;

use reqwest::header::{HeaderMap, RETRY_AFTER};

use crate::engine::{EngineSearchResult, SearchEngine, SearchEngineError};

/// Maximum results per request — the documented cap for both the Brave and
/// Tavily search APIs.
pub(crate) const MAX_API_COUNT: usize = 20;

/// A single organic result normalized across the API backends (Brave and
/// Tavily), ready for shared parsing.
pub(crate) struct OrganicItem {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub score: f64,
    pub published_date: Option<String>,
}

/// Parse an iterator of [`OrganicItem`]s into normalized
/// [`EngineSearchResult`]s, returning up to `count` results with 1-based
/// positions.
///
/// Entries missing a title or URL are skipped; a missing snippet is allowed
/// and maps to an empty snippet. Shared by the Brave and Tavily API backends,
/// which differ only in the JSON field that carries the snippet.
pub(crate) fn parse_organic_results(
    items: impl IntoIterator<Item = OrganicItem>,
    count: usize,
    engine: SearchEngine,
) -> Vec<EngineSearchResult> {
    let mut out = Vec::new();
    for (position, result) in items.into_iter().enumerate() {
        if out.len() >= count {
            break;
        }
        let title = result.title.trim().to_string();
        let url = result.url.trim().to_string();
        if title.is_empty() || url.is_empty() {
            continue;
        }
        out.push(EngineSearchResult {
            title,
            url,
            snippet: result.snippet.trim().to_string(),
            position: position + 1,
            engine,
            score: result.score,
            published_date: result.published_date,
            favicon: None,
        });
    }
    out
}

/// Parse the `Retry-After` header into milliseconds to wait before retrying.
///
/// Only the delay-seconds form is supported; an HTTP-date (or a missing or
/// unparseable header) yields `None` so the caller falls back to its own
/// backoff.
pub(crate) fn retry_after_ms(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(|secs| secs.saturating_mul(1000))
}

/// Build the shared rate-limit/auth error for a status, or `None` when the
/// status is neither 429 nor 401/403. `rate_limit_detail` builds the 429
/// detail string from the response headers (Brave uses `X-RateLimit-*`
/// headers; Tavily echoes `Retry-After`).
pub(crate) fn rate_limit_or_auth_error(
    engine: SearchEngine,
    status: reqwest::StatusCode,
    headers: &HeaderMap,
    rate_limit_detail: impl FnOnce(&HeaderMap) -> String,
) -> Option<SearchEngineError> {
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Some(SearchEngineError::RateLimited {
            engine,
            detail: rate_limit_detail(headers),
            retry_after_ms: retry_after_ms(headers),
        });
    }
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Some(SearchEngineError::Unavailable {
            engine,
            detail: format!("authentication failed: HTTP {status}"),
        });
    }
    None
}
