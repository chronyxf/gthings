//! Tavily Search API backend (paid, plain HTTP, no browser).
//!
//! POSTs `{"query", "max_results", "search_depth": "basic"}` to
//! `https://api.tavily.com/search` with an `Authorization: Bearer <key>`
//! header and parses the `results[]` array (`.title`, `.url`, `.content`).
//!
//! The API key is resolved per request from `TAVILY_API_KEY` (fallback
//! `GTHINGS_TAVILY_API_KEY`); without a key the backend reports
//! [`SearchEngineError::Unavailable`] so the router can fall back to a free
//! engine. HTTP 429 is mapped to [`SearchEngineError::RateLimited`] honoring
//! the `Retry-After` header; HTTP 401/403 are authentication failures. Other
//! non-success statuses surface the JSON `{"detail":{"error":"..."}}` message
//! when the body carries one.

use reqwest::header::{HeaderMap, RETRY_AFTER};

use crate::engine::{
    EngineSearchResult, SearchEngine, SearchEngineBackend, SearchEngineError, SearchOptions,
    env_var_from,
};

/// Tavily Search API endpoint.
const ENDPOINT: &str = "https://api.tavily.com/search";

/// Env vars consulted for the API key, in precedence order.
const KEY_ENV_VARS: [&str; 2] = ["TAVILY_API_KEY", "GTHINGS_TAVILY_API_KEY"];

/// Stateless Tavily Search API backend.
pub struct TavilyBackend;

/// POST body. `search_depth` defaults to `"basic"` (faster, cheaper) but is
/// overridable per request; `freshness` is optional and only serialized when
/// set.
#[derive(Debug, serde::Serialize)]
struct TavilyRequest<'a> {
    query: &'a str,
    max_results: usize,
    search_depth: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    freshness: Option<&'a str>,
}

impl<'a> TavilyRequest<'a> {
    fn new(query: &'a str, max_results: usize, options: &'a SearchOptions) -> Self {
        Self {
            query,
            max_results,
            search_depth: options.search_depth.as_deref().unwrap_or("basic"),
            freshness: options.freshness.as_deref(),
        }
    }
}

/// Top-level Tavily response envelope.
#[derive(Debug, serde::Deserialize)]
struct TavilyResponse {
    #[serde(default)]
    results: Vec<TavilyResult>,
}

/// A single organic result.
#[derive(Debug, serde::Deserialize)]
struct TavilyResult {
    title: String,
    url: String,
    #[serde(default)]
    content: String,
    /// Relevance score (0.0–1.0) supplied by Tavily.
    #[serde(default)]
    score: f64,
    /// Publication date supplied by Tavily, when present.
    #[serde(default)]
    published_date: Option<String>,
}

/// Error envelope: `{"detail":{"error":"..."}}`.
#[derive(Debug, serde::Deserialize)]
struct TavilyErrorBody {
    #[serde(default)]
    detail: Option<ErrorDetail>,
}

#[derive(Debug, serde::Deserialize)]
struct ErrorDetail {
    #[serde(default)]
    error: Option<String>,
}

/// Resolve the API key from the process environment.
///
/// `TAVILY_API_KEY` takes precedence over `GTHINGS_TAVILY_API_KEY`; unset or
/// empty values are skipped. A missing key is [`SearchEngineError::Unavailable`]
/// so the router falls back to a free engine instead of failing the query.
fn api_key(engine: SearchEngine) -> Result<String, SearchEngineError> {
    env_var_from(&KEY_ENV_VARS, std::env::vars()).ok_or_else(|| SearchEngineError::Unavailable {
        engine,
        detail: "no Tavily API key: set TAVILY_API_KEY or GTHINGS_TAVILY_API_KEY".to_string(),
    })
}

/// Map a non-success Tavily response to a [`SearchEngineError`].
///
/// 429 is rate limiting: the `Retry-After` header (delay-seconds or an
/// HTTP-date) is echoed into the detail so the router can schedule the retry.
/// 401/403 are authentication failures. Any other status is an engine-level
/// failure carrying the JSON `{"detail":{"error":"..."}}` message when present.
fn map_error(
    engine: SearchEngine,
    status: reqwest::StatusCode,
    headers: &HeaderMap,
    body: &str,
) -> SearchEngineError {
    if let Some(err) = super::rate_limit_or_auth_error(engine, status, headers, |h| {
        let retry_after = h
            .get(RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .map(|v| format!("; Retry-After: {v}"))
            .unwrap_or_default();
        format!("HTTP 429 Too Many Requests{retry_after}")
    }) {
        return err;
    }
    let detail = extract_error_detail(body).unwrap_or_else(|| format!("unexpected HTTP {status}"));
    SearchEngineError::Unavailable { engine, detail }
}

/// Extract the message from a Tavily error body: `{"detail":{"error":"..."}}`.
fn extract_error_detail(body: &str) -> Option<String> {
    let parsed: TavilyErrorBody = serde_json::from_str(body).ok()?;
    let message = parsed.detail?.error?;
    let message = message.trim();
    (!message.is_empty()).then(|| message.to_string())
}

/// Parse `results[]` from a Tavily response body, returning up to `count`
/// normalized results with 1-based positions.
///
/// Entries missing a title or URL are skipped; a missing content is allowed
/// and maps to an empty snippet. An empty `results` array is a legitimate
/// "no results" answer.
pub(crate) fn parse_results(
    body: &str,
    count: usize,
    engine: SearchEngine,
) -> Result<Vec<EngineSearchResult>, SearchEngineError> {
    let parsed: TavilyResponse =
        serde_json::from_str(body).map_err(|e| SearchEngineError::Parse {
            engine,
            detail: format!("invalid Tavily JSON: {e}"),
        })?;

    Ok(super::parse_organic_results(
        parsed.results.into_iter().map(|r| super::OrganicItem {
            title: r.title,
            url: r.url,
            snippet: r.content,
            score: r.score,
            published_date: r.published_date,
        }),
        count,
        engine,
    ))
}

impl SearchEngineBackend for TavilyBackend {
    fn name(&self) -> SearchEngine {
        SearchEngine::Tavily
    }

    async fn search(
        &self,
        query: &str,
        count: usize,
        options: &SearchOptions,
    ) -> Result<Vec<EngineSearchResult>, SearchEngineError> {
        let engine = self.name();
        let key = api_key(engine)?;
        let count = count.min(super::MAX_API_COUNT);

        let body =
            serde_json::to_string(&TavilyRequest::new(query, count, options)).map_err(|e| {
                SearchEngineError::Network {
                    engine,
                    detail: format!("failed to serialize request: {e}"),
                }
            })?;

        let resp = crate::engine::send_and_map(
            engine,
            crate::engine::http_client()
                .post(ENDPOINT)
                .header("Authorization", format!("Bearer {key}"))
                .header("Content-Type", "application/json")
                .body(body)
                .send(),
        )
        .await?;

        let status = resp.status();
        if !status.is_success() {
            let headers = resp.headers().clone();
            let body = resp.text().await.unwrap_or_default();
            return Err(map_error(engine, status, &headers, &body));
        }

        let body = resp.text().await.map_err(|e| SearchEngineError::Network {
            engine,
            detail: format!("failed to read response body: {e}"),
        })?;

        let results = parse_results(&body, count, engine)?;
        tracing::debug!("tavily: {query} -> {} results", results.len());
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Placeholder engine label (see `TavilyBackend::name`).
    const ENGINE: SearchEngine = SearchEngine::Tavily;

    const SAMPLE: &str = r#"{
      "query": "rust",
      "results": [
        {
          "title": "Rust Programming Language",
          "url": "https://rust-lang.org/",
          "content": "A language empowering everyone to build reliable and efficient software."
        },
        {
          "title": "Rust (programming language) - Wikipedia",
          "url": "https://en.wikipedia.org/wiki/Rust_(programming_language)",
          "content": ""
        }
      ]
    }"#;

    #[test]
    fn parses_results() {
        let results = parse_results(SAMPLE, 10, ENGINE).expect("sample should parse");
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Rust Programming Language");
        assert_eq!(results[0].url, "https://rust-lang.org/");
        assert_eq!(
            results[0].snippet,
            "A language empowering everyone to build reliable and efficient software."
        );
        assert_eq!(results[0].position, 1);
        assert_eq!(results[0].engine, SearchEngine::Tavily);
    }

    #[test]
    fn empty_content_maps_to_empty_snippet() {
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
        let body = r#"{"results":[
          {"title":"","url":"https://no-title.example/","content":"missing title"},
          {"title":"No URL","url":"","content":"missing url"},
          {"title":"Good","url":"https://good.example/","content":"ok"}
        ]}"#;
        let results = parse_results(body, 10, ENGINE).expect("fixture should parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Good");
    }

    #[test]
    fn empty_results_is_empty_result_set() {
        let results = parse_results(r#"{"results":[]}"#, 10, ENGINE).expect("empty array parses");
        assert!(results.is_empty());
    }

    #[test]
    fn malformed_json_is_parse_error() {
        let err = parse_results("not json", 10, ENGINE).expect_err("garbage must fail");
        assert!(matches!(err, SearchEngineError::Parse { .. }));
    }

    #[test]
    fn maps_429_to_rate_limited_honoring_retry_after() {
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, "45".parse().unwrap());
        let err = map_error(ENGINE, reqwest::StatusCode::TOO_MANY_REQUESTS, &headers, "");
        match err {
            SearchEngineError::RateLimited { engine, detail, .. } => {
                assert_eq!(engine, SearchEngine::Tavily);
                assert!(detail.contains("Retry-After: 45"), "detail: {detail}");
            }
            other => panic!("expected RateLimited, got {other:?}"),
        }
    }

    #[test]
    fn maps_429_without_retry_after_to_rate_limited() {
        let err = map_error(
            ENGINE,
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            &HeaderMap::new(),
            "",
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
        let err = map_error(
            ENGINE,
            reqwest::StatusCode::UNAUTHORIZED,
            &HeaderMap::new(),
            "",
        );
        match err {
            SearchEngineError::Unavailable { engine, detail } => {
                assert_eq!(engine, SearchEngine::Tavily);
                assert!(detail.contains("401"), "detail: {detail}");
            }
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    #[test]
    fn surfaces_detail_error_message_from_body() {
        let body = r#"{"detail":{"error":"query is too short"}}"#;
        let err = map_error(
            ENGINE,
            reqwest::StatusCode::BAD_REQUEST,
            &HeaderMap::new(),
            body,
        );
        match err {
            SearchEngineError::Unavailable { detail, .. } => {
                assert!(detail.contains("query is too short"), "detail: {detail}");
            }
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    #[test]
    fn falls_back_to_status_when_body_has_no_detail() {
        let err = map_error(
            ENGINE,
            reqwest::StatusCode::BAD_GATEWAY,
            &HeaderMap::new(),
            "<html>",
        );
        assert!(matches!(err, SearchEngineError::Unavailable { .. }));
    }

    #[test]
    fn request_body_matches_api_contract() {
        let body =
            serde_json::to_string(&TavilyRequest::new("rust", 10, &SearchOptions::default()))
                .unwrap();
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(value["query"], "rust");
        assert_eq!(value["max_results"], 10);
        assert_eq!(value["search_depth"], "basic");
        assert!(
            value.get("freshness").is_none(),
            "freshness omitted when unset"
        );
    }

    #[test]
    fn request_body_threads_freshness_and_search_depth() {
        let options = SearchOptions {
            freshness: Some("week".to_string()),
            search_depth: Some("advanced".to_string()),
        };
        let body = serde_json::to_string(&TavilyRequest::new("rust", 10, &options)).unwrap();
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(value["search_depth"], "advanced");
        assert_eq!(value["freshness"], "week");
    }

    #[test]
    fn api_key_prefers_tavily_then_gthings() {
        let names = &KEY_ENV_VARS;
        let vars = vec![
            ("TAVILY_API_KEY".to_string(), "k1".to_string()),
            ("GTHINGS_TAVILY_API_KEY".to_string(), "k2".to_string()),
        ];
        assert_eq!(env_var_from(names, vars), Some("k1".to_string()));

        let vars = vec![("GTHINGS_TAVILY_API_KEY".to_string(), "k2".to_string())];
        assert_eq!(env_var_from(names, vars), Some("k2".to_string()));

        let vars = vec![("TAVILY_API_KEY".to_string(), "".to_string())];
        assert_eq!(env_var_from(names, vars), None);

        assert_eq!(env_var_from(names, Vec::<(String, String)>::new()), None);
    }
}
