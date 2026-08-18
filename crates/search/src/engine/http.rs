//! Shared, lazily-built HTTP infrastructure for the plain-HTTP engine
//! backends (scrape/bing, api/brave, api/tavily).

use std::future::Future;
use std::sync::OnceLock;

use reqwest::header::{ACCEPT_LANGUAGE, USER_AGENT};

use crate::engine::{SearchEngine, SearchEngineError};

/// Default headers for the plain-HTTP backends (scrape/bing, api/brave,
/// api/tavily): the configurable User-Agent (see
/// [`gthings_common::user_agent::gthings_agent`]: honors `GTHINGS_AGENT`, else
/// the compile-time stamp) and `Accept-Language: en-US`.
fn default_headers() -> reqwest::header::HeaderMap {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        USER_AGENT,
        reqwest::header::HeaderValue::from_str(&gthings_common::user_agent::gthings_agent())
            .expect("resolved user-agent is a valid header value"),
    );
    headers.insert(
        ACCEPT_LANGUAGE,
        reqwest::header::HeaderValue::from_static("en-US,en;q=0.9"),
    );
    headers
}

/// Shared, lazily-built HTTP client for plain-HTTP backends (scrape/bing,
/// api/brave, api/tavily).
///
/// Configured with [`default_headers`], plus a 15-second timeout per request.
pub(crate) fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .default_headers(default_headers())
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .expect("failed to build shared HTTP client")
    })
}

/// Send a request future and map a transport-level failure onto
/// [`SearchEngineError::Network`].
///
/// Shared by the plain-HTTP backends (scrape/bing, api/brave, api/tavily),
/// which all report a failed `.send()` identically.
pub(crate) async fn send_and_map(
    engine: SearchEngine,
    fut: impl Future<Output = Result<reqwest::Response, reqwest::Error>>,
) -> Result<reqwest::Response, SearchEngineError> {
    fut.await.map_err(|e| SearchEngineError::Network {
        engine,
        detail: format!("request failed: {e}"),
    })
}

#[cfg(test)]
mod tests {
    use super::default_headers;
    use reqwest::header::USER_AGENT;

    /// The shared client's default headers must carry the configurable agent
    /// string, not a hardcoded constant. `gthings_agent()` is process-cached,
    /// so the header always matches the resolved value regardless of
    /// `GTHINGS_AGENT`.
    #[test]
    fn http_client_uses_configurable_user_agent() {
        let headers = default_headers();
        let ua = headers
            .get(USER_AGENT)
            .expect("client sets a User-Agent header")
            .to_str()
            .unwrap();
        assert_eq!(ua, gthings_common::user_agent::gthings_agent());
        assert!(!ua.is_empty());
    }
}
