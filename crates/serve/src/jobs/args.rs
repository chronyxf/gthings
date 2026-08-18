//! Per-op argument parsing and validation.
//!
//! `POST /job` bodies carry a raw `args` JSON value; [`JobArgs::parse`]
//! validates it against the operation and returns a typed [`JobArgs`]. Every
//! validation failure surfaces as an `invalid-input` [`ErrorBody`] so the HTTP
//! layer can answer 4xx with the canonical taxonomy code.

use gthings_common::envelope::ErrorBody;
use gthings_common::taxonomy::ErrorCode;
use gthings_search::{EngineChoice, SearchEngine};
use serde::Deserialize;

use super::Op;

/// Default result count when `count` is absent.
pub(crate) const DEFAULT_COUNT: usize = 5;
/// Minimum accepted result count.
pub(crate) const MIN_COUNT: usize = 1;
/// Maximum accepted result count.
pub(crate) const MAX_COUNT: usize = 100;
/// Default number of results followed by a `harvest` run.
pub(crate) const DEFAULT_FOLLOW_TOP: usize = 8;
/// Maximum accepted `follow_top`.
pub(crate) const MAX_FOLLOW_TOP: usize = 50;
/// Maximum accepted query-list length — bounds per-job fan-out so a single
/// `POST /job` cannot bypass the daemon's concurrency cap.
pub(crate) const MAX_QUERIES: usize = 16;

/// Research strategy, mirroring the CLI `--strategy` flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Strategy {
    /// Single-query search.
    Simple,
    /// Multi-query search, one entry per query.
    Parallel,
    /// Full pipeline: search → dedup → rank → follow → quality.
    Harvest,
    /// Non-search op (`extract`/`ax`/`pdf-url`/`pdf-file`); not dispatched to
    /// the search core. Internal only — not parseable from user input.
    NonSearch,
}

impl Strategy {
    /// Parse a strategy string, case-insensitive.
    #[must_use]
    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "simple" => Some(Self::Simple),
            "parallel" => Some(Self::Parallel),
            "harvest" => Some(Self::Harvest),
            _ => None,
        }
    }
}

/// `engine` argument accepted by the wire contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum EngineArg {
    /// Let the router pick engines in priority order.
    Auto,
    Brave,
    Bing,
    Google,
}

impl EngineArg {
    /// Parse an engine string, case-insensitive.
    #[must_use]
    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "brave" => Some(Self::Brave),
            "bing" => Some(Self::Bing),
            "google" => Some(Self::Google),
            _ => None,
        }
    }

    /// Map to the search core's [`EngineChoice`].
    #[must_use]
    pub(crate) fn to_engine_choice(self) -> EngineChoice {
        match self {
            Self::Auto => EngineChoice::Auto,
            Self::Brave => EngineChoice::Pin(SearchEngine::Brave),
            Self::Bing => EngineChoice::Pin(SearchEngine::Bing),
            Self::Google => EngineChoice::Pin(SearchEngine::Google),
        }
    }
}

/// Raw, unvalidated wire arguments mirroring the CLI flags.
#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct RawArgs {
    /// Single query (`simple`).
    pub query: Option<String>,
    /// Query list (`parallel`, `harvest`).
    #[serde(default)]
    pub queries: Vec<String>,
    /// Max results per query; default [`DEFAULT_COUNT`].
    pub count: Option<usize>,
    /// Strategy; must match the op when present.
    pub strategy: Option<String>,
    /// Engine override; default `auto`.
    pub engine: Option<String>,
    /// Number of results followed by `harvest`; default [`DEFAULT_FOLLOW_TOP`].
    pub follow_top: Option<usize>,
    /// Target URL for `extract`/`ax`/`pdf-url`.
    pub url: Option<String>,
    /// Local file path for `pdf-file`.
    pub path: Option<String>,
    /// Character cap for `extract`; unlimited when absent.
    pub max_chars: Option<usize>,
    /// Recency filter (`day`/`week`/`month`/`year` or an ISO date).
    pub freshness: Option<String>,
    /// Search depth (`basic`/`advanced`).
    pub search_depth: Option<String>,
}

/// Validated arguments, ready to drive the search core.
///
/// [`Hash`] is derived so [`JobArgs`] can be used as a map key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct JobArgs {
    /// Single query for `simple`; `None` for multi-query ops.
    pub query: Option<String>,
    /// Non-empty query list for `parallel`/`harvest`.
    pub queries: Vec<String>,
    /// Max results per query (1–[`MAX_COUNT`]).
    pub count: usize,
    /// Validated strategy (always consistent with the op).
    pub strategy: Strategy,
    /// Engine override (default `auto`).
    pub engine: EngineArg,
    /// Followed results for `harvest` (1–[`MAX_FOLLOW_TOP`]).
    pub follow_top: usize,
    /// Target URL for `extract`/`ax`/`pdf-url`; `None` otherwise.
    pub url: Option<String>,
    /// Local file path for `pdf-file`; `None` otherwise.
    pub path: Option<String>,
    /// Character cap for `extract`; `None` for unlimited.
    pub max_chars: Option<usize>,
    /// Recency filter (`day`/`week`/`month`/`year` or an ISO date); `None` for
    /// the engine default.
    pub freshness: Option<String>,
    /// Search depth (`basic`/`advanced`); `None` for the engine default.
    pub search_depth: Option<String>,
}

impl JobArgs {
    /// Deserialize `raw` and validate it against `op`.
    ///
    /// # Errors
    ///
    /// Returns an `invalid-input` [`ErrorBody`] for malformed JSON or any
    /// rule violation (missing query/queries, missing/invalid url, missing
    /// path, op/strategy mismatch, unknown strategy/engine, out-of-range
    /// count/follow_top/max_chars).
    pub(crate) fn parse(op: Op, raw: &serde_json::Value) -> Result<Self, ErrorBody> {
        let raw: RawArgs = serde_json::from_value(raw.clone())
            .map_err(|e| invalid_input(format!("malformed args: {e}")))?;
        Self::validate(op, raw)
    }

    fn validate(op: Op, raw: RawArgs) -> Result<Self, ErrorBody> {
        let expected = default_strategy(op);

        let strategy = match raw.strategy.as_deref() {
            Some(s) => {
                Strategy::parse(s).ok_or_else(|| invalid_input(format!("unknown strategy: {s}")))?
            }
            None => expected,
        };
        if strategy != expected {
            return Err(invalid_input(format!(
                "strategy '{strategy:?}' is not valid for op '{op}'"
            )));
        }

        let count = raw.count.unwrap_or(DEFAULT_COUNT);
        if !(MIN_COUNT..=MAX_COUNT).contains(&count) {
            return Err(invalid_input(format!(
                "count {count} out of range {MIN_COUNT}..={MAX_COUNT}"
            )));
        }

        let follow_top = raw.follow_top.unwrap_or(DEFAULT_FOLLOW_TOP);
        if !(1..=MAX_FOLLOW_TOP).contains(&follow_top) {
            return Err(invalid_input(format!(
                "follow_top {follow_top} out of range 1..={MAX_FOLLOW_TOP}"
            )));
        }

        let engine = match raw.engine.as_deref() {
            Some(s) => {
                EngineArg::parse(s).ok_or_else(|| invalid_input(format!("unknown engine: {s}")))?
            }
            None => EngineArg::Auto,
        };

        let freshness = match raw.freshness.as_deref() {
            Some(s) => {
                let s = s.trim();
                if !matches!(s, "day" | "week" | "month" | "year") && !is_date(s) {
                    return Err(invalid_input(format!(
                        "freshness '{s}' must be day/week/month/year or an ISO date (YYYY-MM-DD)"
                    )));
                }
                Some(s.to_string())
            }
            None => None,
        };

        let search_depth = match raw.search_depth.as_deref() {
            Some(s) => {
                let s = s.trim();
                if !matches!(s, "basic" | "advanced") {
                    return Err(invalid_input(format!(
                        "search_depth '{s}' must be basic or advanced"
                    )));
                }
                Some(s.to_string())
            }
            None => None,
        };

        let query = raw
            .query
            .as_deref()
            .map(str::trim)
            .filter(|q| !q.is_empty());
        let queries: Vec<String> = raw
            .queries
            .into_iter()
            .map(|q| q.trim().to_string())
            .filter(|q| !q.is_empty())
            .collect();
        let url = raw.url.as_deref().map(str::trim).filter(|u| !u.is_empty());
        let path = raw.path.as_deref().map(str::trim).filter(|p| !p.is_empty());

        match op {
            Op::Simple => {
                let query = query
                    .ok_or_else(|| invalid_input("op 'simple' requires a non-empty 'query'"))?;
                if !queries.is_empty() {
                    return Err(invalid_input(
                        "op 'simple' does not accept 'queries'; use 'query'",
                    ));
                }
                Ok(Self {
                    query: Some(query.to_string()),
                    queries,
                    count,
                    strategy,
                    engine,
                    follow_top,
                    url: None,
                    path: None,
                    max_chars: None,
                    freshness,
                    search_depth,
                })
            }
            Op::Parallel | Op::Harvest => {
                if query.is_some() {
                    return Err(invalid_input(format!(
                        "op '{op}' does not accept 'query'; use 'queries'"
                    )));
                }
                if queries.is_empty() {
                    return Err(invalid_input(format!(
                        "op '{op}' requires a non-empty 'queries' array"
                    )));
                }
                if queries.len() > MAX_QUERIES {
                    return Err(invalid_input(format!(
                        "op '{op}' accepts at most {MAX_QUERIES} queries, got {}",
                        queries.len()
                    )));
                }
                Ok(Self {
                    query: None,
                    queries,
                    count,
                    strategy,
                    engine,
                    follow_top,
                    url: None,
                    path: None,
                    max_chars: None,
                    freshness,
                    search_depth,
                })
            }
            Op::Extract | Op::Ax | Op::PdfUrl | Op::PdfFile => {
                if query.is_some() || !queries.is_empty() {
                    return Err(invalid_input(format!(
                        "op '{op}' does not accept 'query'/'queries'"
                    )));
                }
                let max_chars = if op == Op::Extract {
                    match raw.max_chars {
                        Some(0) => {
                            return Err(invalid_input("op 'extract' max_chars must be positive"));
                        }
                        Some(n) => Some(n),
                        None => None,
                    }
                } else {
                    None
                };
                match op {
                    Op::Extract | Op::Ax | Op::PdfUrl => {
                        let url = url.ok_or_else(|| {
                            invalid_input(format!("op '{op}' requires a non-empty 'url'"))
                        })?;
                        if !is_http_url(url) {
                            return Err(invalid_input(format!(
                                "op '{op}' url '{url}' must use the http:// or https:// scheme"
                            )));
                        }
                        Ok(Self {
                            query: None,
                            queries,
                            count,
                            strategy,
                            engine,
                            follow_top,
                            url: Some(url.to_string()),
                            path: None,
                            max_chars,
                            freshness,
                            search_depth,
                        })
                    }
                    Op::PdfFile => {
                        let path = path.ok_or_else(|| {
                            invalid_input("op 'pdf-file' requires a non-empty 'path'")
                        })?;
                        Ok(Self {
                            query: None,
                            queries,
                            count,
                            strategy,
                            engine,
                            follow_top,
                            url: None,
                            path: Some(path.to_string()),
                            max_chars: None,
                            freshness,
                            search_depth,
                        })
                    }
                    _ => unreachable!("outer arm covers all non-search ops"),
                }
            }
        }
    }
}

/// The strategy an op implies by default.
fn default_strategy(op: Op) -> Strategy {
    match op {
        Op::Simple => Strategy::Simple,
        Op::Parallel => Strategy::Parallel,
        Op::Harvest => Strategy::Harvest,
        Op::Extract | Op::Ax | Op::PdfUrl | Op::PdfFile => Strategy::NonSearch,
    }
}

/// Whether `url` is an absolute `http://` or `https://` URL with a host.
#[must_use]
fn is_http_url(url: &str) -> bool {
    url.strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .is_some_and(|rest| !rest.is_empty())
}

/// Whether `s` is an ISO date of the form `YYYY-MM-DD` (numeric components).
#[must_use]
fn is_date(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

/// Build an `invalid-input` error body. Shared with the API layer so the
/// canonical taxonomy code is produced in exactly one place.
pub(crate) fn invalid_input(detail: impl Into<String>) -> ErrorBody {
    ErrorBody::new(ErrorCode::InvalidInput, detail)
}

#[cfg(test)]
mod tests {
    use gthings_common::taxonomy::ErrorCode;
    use serde_json::json;

    use super::{EngineArg, JobArgs, MAX_QUERIES, Op, Strategy};

    fn parse(op: Op, args: serde_json::Value) -> Result<JobArgs, gthings_common::ErrorBody> {
        JobArgs::parse(op, &args)
    }

    #[test]
    fn simple_requires_query() {
        let err = parse(Op::Simple, json!({})).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn simple_rejects_blank_query() {
        let err = parse(Op::Simple, json!({"query": "   "})).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn simple_rejects_queries_field() {
        let err = parse(Op::Simple, json!({"query": "ok", "queries": ["a"]})).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn simple_valid_with_defaults() {
        let args = parse(Op::Simple, json!({"query": "  rust async  "})).unwrap();
        assert_eq!(args.query.as_deref(), Some("rust async"));
        assert_eq!(args.count, 5);
        assert_eq!(args.strategy, Strategy::Simple);
        assert_eq!(args.engine, EngineArg::Auto);
    }

    #[test]
    fn parallel_requires_queries() {
        let err = parse(Op::Parallel, json!({"query": "x"})).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
        let err = parse(Op::Parallel, json!({"queries": []})).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn parallel_rejects_single_query_field() {
        let err = parse(Op::Parallel, json!({"queries": ["a"], "query": "x"})).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn parallel_valid_filters_blank_queries() {
        let args = parse(Op::Parallel, json!({"queries": ["a", " ", "b"]})).unwrap();
        assert_eq!(args.queries, vec!["a", "b"]);
        assert_eq!(args.strategy, Strategy::Parallel);
    }

    #[test]
    fn parallel_and_harvest_cap_query_count() {
        let many: Vec<String> = (0..=MAX_QUERIES).map(|i| format!("q{i}")).collect();
        let err = parse(Op::Parallel, json!({"queries": many})).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);

        let ok = parse(Op::Harvest, json!({"queries": ["a", "b"]})).unwrap();
        assert_eq!(ok.queries.len(), 2);
    }

    #[test]
    fn harvest_valid() {
        let args = parse(
            Op::Harvest,
            json!({"queries": ["a"], "count": 10, "follow_top": 4, "engine": "brave"}),
        )
        .unwrap();
        assert_eq!(args.queries, vec!["a"]);
        assert_eq!(args.count, 10);
        assert_eq!(args.follow_top, 4);
        assert_eq!(args.engine, EngineArg::Brave);
        assert_eq!(args.strategy, Strategy::Harvest);
    }

    #[test]
    fn strategy_must_match_op() {
        let err = parse(Op::Simple, json!({"query": "x", "strategy": "harvest"})).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
        let err = parse(Op::Harvest, json!({"queries": ["a"], "strategy": "simple"})).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn unknown_strategy_and_engine_rejected() {
        let err = parse(Op::Simple, json!({"query": "x", "strategy": "deep"})).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
        let err = parse(Op::Simple, json!({"query": "x", "engine": "yahoo"})).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn count_and_follow_top_ranges_enforced() {
        let err = parse(Op::Simple, json!({"query": "x", "count": 0})).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
        let err = parse(Op::Simple, json!({"query": "x", "count": 101})).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
        let err = parse(Op::Simple, json!({"query": "x", "follow_top": 0})).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
        let err = parse(Op::Simple, json!({"query": "x", "follow_top": 51})).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn malformed_args_json_rejected() {
        let err = parse(Op::Simple, json!({"query": ["not-a-string"]})).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn engine_choice_mapping() {
        assert_eq!(
            EngineArg::Auto.to_engine_choice(),
            gthings_search::EngineChoice::Auto
        );
        assert_eq!(
            EngineArg::Brave.to_engine_choice(),
            gthings_search::EngineChoice::Pin(gthings_search::SearchEngine::Brave)
        );
    }

    #[test]
    fn extract_requires_http_url() {
        let err = parse(Op::Extract, json!({})).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
        let err = parse(Op::Extract, json!({"url": "  "})).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
        let err = parse(Op::Extract, json!({"url": "example.com/a"})).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
        let err = parse(Op::Extract, json!({"url": "ftp://example.com/a"})).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
        let err = parse(Op::Extract, json!({"url": "http://"})).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn extract_valid_with_optional_max_chars() {
        let args = parse(Op::Extract, json!({"url": "https://example.com/a"})).unwrap();
        assert_eq!(args.url.as_deref(), Some("https://example.com/a"));
        assert_eq!(args.max_chars, None);

        let args = parse(
            Op::Extract,
            json!({"url": "  http://example.com/a  ", "max_chars": 4000}),
        )
        .unwrap();
        assert_eq!(args.url.as_deref(), Some("http://example.com/a"));
        assert_eq!(args.max_chars, Some(4000));

        let err = parse(Op::Extract, json!({"url": "https://a.com", "max_chars": 0})).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn ax_requires_http_url() {
        let err = parse(Op::Ax, json!({})).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
        let err = parse(Op::Ax, json!({"url": "mailto:x@y"})).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn ax_valid() {
        let args = parse(Op::Ax, json!({"url": "https://example.com"})).unwrap();
        assert_eq!(args.url.as_deref(), Some("https://example.com"));
        assert_eq!(args.path, None);
        assert_eq!(args.max_chars, None);
        assert_eq!(args.strategy, Strategy::NonSearch);
    }

    #[test]
    fn pdf_url_requires_http_url() {
        let err = parse(Op::PdfUrl, json!({})).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
        let err = parse(Op::PdfUrl, json!({"url": "file:///tmp/a.pdf"})).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn pdf_url_valid() {
        let args = parse(Op::PdfUrl, json!({"url": "https://example.com/doc.pdf"})).unwrap();
        assert_eq!(args.url.as_deref(), Some("https://example.com/doc.pdf"));
        assert_eq!(args.max_chars, None);
    }

    #[test]
    fn pdf_file_requires_path() {
        let err = parse(Op::PdfFile, json!({})).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
        let err = parse(Op::PdfFile, json!({"path": "   "})).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
        let err = parse(Op::PdfFile, json!({"url": "https://example.com/a.pdf"})).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn pdf_file_valid() {
        let args = parse(Op::PdfFile, json!({"path": "/tmp/report.pdf"})).unwrap();
        assert_eq!(args.path.as_deref(), Some("/tmp/report.pdf"));
        assert_eq!(args.url, None);
        assert_eq!(args.strategy, Strategy::NonSearch);
    }

    #[test]
    fn non_search_ops_reject_query_fields() {
        let err = parse(
            Op::Extract,
            json!({"url": "https://example.com", "query": "x"}),
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
        let err = parse(Op::PdfFile, json!({"path": "/tmp/a.pdf", "queries": ["a"]})).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn non_search_ops_reject_search_strategies() {
        let err = parse(
            Op::Extract,
            json!({"url": "https://example.com", "strategy": "harvest"}),
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }
}
