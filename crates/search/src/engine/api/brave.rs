//! Brave Search API backend (paid, plain HTTP, no browser).
//!
//! Calls `GET https://api.search.brave.com/res/v1/web/search` with the
//! `X-Subscription-Token` header and parses the JSON `web.results[]` array.
//! Unlike the scraper backend ([`crate::engine::scrape::brave`]), API results carry `.title`,
//! `.url`, and `.description` fields, so parsing is a straight JSON decode —
//! no HTML block extraction.
//!
//! The subscription key is resolved per request from `BRAVE_API_KEY`
//! (fallback `GTHINGS_BRAVE_API_KEY`); without a key the backend reports
//! [`SearchEngineError::Unavailable`] so the router can fall back to a free
//! engine. HTTP 429 is mapped to [`SearchEngineError::RateLimited`] using only
//! the status line and `X-RateLimit-*` response headers (the 429 body is not
//! JSON and is never parsed); HTTP 401/403 are authentication failures.

use reqwest::header::HeaderMap;

use crate::engine::{
    EngineSearchResult, SearchEngine, SearchEngineBackend, SearchEngineError, SearchOptions,
    env_var_from,
};

/// Brave Search API endpoint.
const ENDPOINT: &str = "https://api.search.brave.com/res/v1/web/search";

/// Auth header carrying the subscription key.
const AUTH_HEADER: &str = "X-Subscription-Token";

/// Env vars consulted for the subscription key, in precedence order.
const KEY_ENV_VARS: [&str; 2] = ["BRAVE_API_KEY", "GTHINGS_BRAVE_API_KEY"];

/// Stateless Brave Search API backend.
pub struct BraveApiBackend;

/// Resolve the subscription key from the process environment.
///
/// `BRAVE_API_KEY` takes precedence over `GTHINGS_BRAVE_API_KEY`; unset or
/// empty values are skipped. A missing key is [`SearchEngineError::Unavailable`]
/// so the router falls back to a free engine instead of failing the query.
fn api_key() -> Result<String, SearchEngineError> {
    env_var_from(&KEY_ENV_VARS, std::env::vars()).ok_or_else(|| SearchEngineError::Unavailable {
        engine: SearchEngine::BraveApi,
        detail: "no Brave API key: set BRAVE_API_KEY or GTHINGS_BRAVE_API_KEY".to_string(),
    })
}

/// Build the Brave API search URL: endpoint plus `q`, `count`, `country`,
/// and `search_lang` query parameters (pinned to the English interface).
/// `freshness` is appended when set.
fn build_url(query: &str, count: usize, freshness: Option<&str>) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    serializer
        .append_pair("q", query)
        .append_pair("count", &count.to_string())
        .append_pair("country", "us")
        .append_pair("search_lang", "en");
    if let Some(freshness) = freshness {
        serializer.append_pair("freshness", freshness);
    }
    let params = serializer.finish();
    format!("{ENDPOINT}?{params}")
}

/// Map a non-success HTTP status to a [`SearchEngineError`].
///
/// 429 is rate limiting: the detail is built purely from the status line and
/// the `X-RateLimit-*` response headers (Brave's 429 body is not JSON and is
/// not read). 401/403 are authentication failures. Any other status is a
/// network-level failure.
fn map_status_error(
    engine: SearchEngine,
    status: reqwest::StatusCode,
    headers: &HeaderMap,
) -> SearchEngineError {
    if let Some(err) = super::rate_limit_or_auth_error(engine, status, headers, rate_limit_detail) {
        return err;
    }
    SearchEngineError::Network {
        engine,
        detail: format!("unexpected HTTP {status}"),
    }
}

/// Build a rate-limit detail string from the `X-RateLimit-*` response headers.
///
/// Falls back to a bare `HTTP 429` when no rate-limit headers are present.
fn rate_limit_detail(headers: &HeaderMap) -> String {
    let mut parts: Vec<String> = Vec::new();
    for (name, value) in headers {
        if name
            .as_str()
            .to_ascii_lowercase()
            .starts_with("x-ratelimit")
        {
            if let Ok(value) = value.to_str() {
                parts.push(format!("{name}={value}"));
            }
        }
    }
    if parts.is_empty() {
        "HTTP 429 Too Many Requests".to_string()
    } else {
        format!("HTTP 429; {}", parts.join(", "))
    }
}

/// Top-level Brave API response envelope.
#[derive(Debug, serde::Deserialize)]
struct BraveApiResponse {
    #[serde(default)]
    web: Option<WebResults>,
}

/// The `web` result group.
#[derive(Debug, serde::Deserialize)]
struct WebResults {
    #[serde(default)]
    results: Vec<WebResult>,
}

/// A single organic web result.
#[derive(Debug, serde::Deserialize)]
struct WebResult {
    title: String,
    url: String,
    #[serde(default)]
    description: String,
    /// Relevance score (0.0–1.0) supplied by Brave, when present.
    #[serde(default)]
    score: f64,
}

/// Parse `web.results[]` from a Brave API response body, returning up to
/// `count` normalized results with 1-based positions.
///
/// Entries missing a title or URL are skipped; a missing description is
/// allowed and maps to an empty snippet. A response without a `web` group —
/// or with an empty `results` array — is a legitimate "no results" answer.
pub(crate) fn parse_results(
    body: &str,
    count: usize,
    engine: SearchEngine,
) -> Result<Vec<EngineSearchResult>, SearchEngineError> {
    let parsed: BraveApiResponse =
        serde_json::from_str(body).map_err(|e| SearchEngineError::Parse {
            engine,
            detail: format!("invalid Brave API JSON: {e}"),
        })?;
    let results = parsed.web.map(|web| web.results).unwrap_or_default();

    Ok(super::parse_organic_results(
        results.into_iter().map(|r| super::OrganicItem {
            title: r.title,
            url: r.url,
            snippet: r.description,
            score: r.score,
            published_date: None,
        }),
        count,
        engine,
    ))
}

impl SearchEngineBackend for BraveApiBackend {
    fn name(&self) -> SearchEngine {
        SearchEngine::BraveApi
    }

    async fn search(
        &self,
        query: &str,
        count: usize,
        options: &SearchOptions,
    ) -> Result<Vec<EngineSearchResult>, SearchEngineError> {
        let engine = self.name();
        let key = api_key()?;
        let count = count.min(super::MAX_API_COUNT);

        let url = build_url(query, count, options.freshness.as_deref());
        let resp = crate::engine::send_and_map(
            engine,
            crate::engine::http_client()
                .get(&url)
                .header(AUTH_HEADER, key)
                .send(),
        )
        .await?;

        let status = resp.status();
        if !status.is_success() {
            return Err(map_status_error(engine, status, resp.headers()));
        }

        let body = resp.text().await.map_err(|e| SearchEngineError::Network {
            engine,
            detail: format!("failed to read response body: {e}"),
        })?;

        let results = parse_results(&body, count, engine)?;
        tracing::debug!("brave-api: {query} -> {} results", results.len());
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    /// Placeholder engine label (see `BraveApiBackend::name`).
    const ENGINE: SearchEngine = SearchEngine::BraveApi;

    const SAMPLE: &str = r#"{
      "web": {
        "results": [
          {
            "title": "Rust Programming Language",
            "url": "https://rust-lang.org/",
            "description": "A language empowering everyone to build reliable and efficient software."
          },
          {
            "title": "Rust (programming language) - Wikipedia",
            "url": "https://en.wikipedia.org/wiki/Rust_(programming_language)",
            "description": ""
          }
        ]
      }
    }"#;

    #[test]
    fn parses_web_results() {
        let results = parse_results(SAMPLE, 10, ENGINE).expect("sample should parse");
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Rust Programming Language");
        assert_eq!(results[0].url, "https://rust-lang.org/");
        assert_eq!(
            results[0].snippet,
            "A language empowering everyone to build reliable and efficient software."
        );
        assert_eq!(results[0].position, 1);
        assert_eq!(results[0].engine, SearchEngine::BraveApi);
    }

    #[test]
    fn empty_description_maps_to_empty_snippet() {
        let results = parse_results(SAMPLE, 10, ENGINE).expect("sample should parse");
        assert_eq!(
            results[1].url,
            "https://en.wikipedia.org/wiki/Rust_(programming_language)"
        );
        assert_eq!(results[1].snippet, "");
    }

    #[test]
    fn respects_count_limit() {
        let results = parse_results(SAMPLE, 1, ENGINE).expect("sample should parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Rust Programming Language");
    }

    #[test]
    fn skips_entries_missing_title_or_url() {
        let body = r#"{"web":{"results":[
          {"title":"","url":"https://no-title.example/","description":"missing title"},
          {"title":"No URL","url":"","description":"missing url"},
          {"title":"Good","url":"https://good.example/","description":"ok"}
        ]}}"#;
        let results = parse_results(body, 10, ENGINE).expect("fixture should parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Good");
    }

    #[test]
    fn empty_web_group_is_empty_result_set() {
        let empty = parse_results(r#"{"web":{"results":[]}}"#, 10, ENGINE)
            .expect("empty array should parse");
        assert!(empty.is_empty());
        let no_web = parse_results(r#"{}"#, 10, ENGINE).expect("no web group should parse");
        assert!(no_web.is_empty());
    }

    #[test]
    fn malformed_json_is_parse_error() {
        let err = parse_results("not json", 10, ENGINE).expect_err("garbage must fail");
        assert!(matches!(err, SearchEngineError::Parse { .. }));
    }

    #[test]
    fn maps_429_to_rate_limited_with_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-ratelimit-remaining"
                .parse::<reqwest::header::HeaderName>()
                .unwrap(),
            HeaderValue::from_static("0"),
        );
        headers.insert(
            "x-ratelimit-reset"
                .parse::<reqwest::header::HeaderName>()
                .unwrap(),
            HeaderValue::from_static("60"),
        );
        let err = map_status_error(ENGINE, reqwest::StatusCode::TOO_MANY_REQUESTS, &headers);
        match err {
            SearchEngineError::RateLimited { engine, detail, .. } => {
                assert_eq!(engine, SearchEngine::BraveApi);
                assert!(
                    detail.contains("x-ratelimit-remaining=0"),
                    "detail: {detail}"
                );
                assert!(detail.contains("x-ratelimit-reset=60"), "detail: {detail}");
            }
            other => panic!("expected RateLimited, got {other:?}"),
        }
    }

    #[test]
    fn maps_429_without_headers_to_rate_limited() {
        let err = map_status_error(
            ENGINE,
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            &HeaderMap::new(),
        );
        match err {
            SearchEngineError::RateLimited { detail, .. } => {
                assert!(detail.contains("HTTP 429"), "detail: {detail}");
            }
            other => panic!("expected RateLimited, got {other:?}"),
        }
    }

    #[test]
    fn maps_401_to_auth_error() {
        let err = map_status_error(ENGINE, reqwest::StatusCode::UNAUTHORIZED, &HeaderMap::new());
        match err {
            SearchEngineError::Unavailable { engine, detail } => {
                assert_eq!(engine, SearchEngine::BraveApi);
                assert!(detail.contains("401"), "detail: {detail}");
            }
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    #[test]
    fn maps_other_status_to_network_error() {
        let err = map_status_error(ENGINE, reqwest::StatusCode::BAD_GATEWAY, &HeaderMap::new());
        assert!(matches!(err, SearchEngineError::Network { .. }));
    }

    #[test]
    fn build_url_includes_all_params() {
        let url = build_url("rust lang", 10, None);
        assert!(url.starts_with("https://api.search.brave.com/res/v1/web/search?"));
        assert!(url.contains("q=rust+lang"), "query must be form-encoded");
        assert!(url.contains("count=10"));
        assert!(url.contains("country=us"));
        assert!(url.contains("search_lang=en"));
        assert!(!url.contains("freshness="), "freshness omitted when unset");
    }

    #[test]
    fn build_url_appends_freshness_when_set() {
        let url = build_url("rust lang", 10, Some("week"));
        assert!(url.contains("freshness=week"), "freshness must be appended");
    }

    #[test]
    fn api_key_prefers_brave_then_gthings() {
        let names = &KEY_ENV_VARS;
        let vars = vec![
            ("BRAVE_API_KEY".to_string(), "k1".to_string()),
            ("GTHINGS_BRAVE_API_KEY".to_string(), "k2".to_string()),
        ];
        assert_eq!(env_var_from(names, vars), Some("k1".to_string()));

        let vars = vec![("GTHINGS_BRAVE_API_KEY".to_string(), "k2".to_string())];
        assert_eq!(env_var_from(names, vars), Some("k2".to_string()));

        let vars = vec![("BRAVE_API_KEY".to_string(), "  ".to_string())];
        assert_eq!(env_var_from(names, vars), None);

        assert_eq!(env_var_from(names, Vec::<(String, String)>::new()), None);
    }
}
