//! Bing search backend via plain HTTP (Bing RSS endpoint).
//!
//! Fetches `https://www.bing.com/search?format=rss&q=...&setlang=en&mkt=
//! en-US` over plain HTTP using the shared crate client. That endpoint
//! serves clean RSS 2.0 — ten
//! `<item>` entries with `title`/`link`/`description` — without a CAPTCHA,
//! so no browser/CDP is required. Block pages (Cloudflare Turnstile
//! interstitials, consent walls, generic challenges) are not RSS feeds; when
//! parsing fails and the body carries such markers, the backend reports
//! [`SearchEngineError::Unavailable`] so the router can fall back to another
//! engine. A well-formed feed with zero items is a legitimate "no results"
//! response and yields an empty vector.

use std::sync::LazyLock;

use regex::Regex;

use super::html::{body_has_block_markers, collapse_whitespace, decode_entities, strip_tags};
use super::{EngineSearchResult, SearchEngine, SearchEngineBackend, SearchEngineError};

/// Stateless Bing backend (plain HTTP, no browser).
#[derive(Default)]
pub struct BingBackend;

impl BingBackend {
    /// Create a backend. Stateless — no browser session is needed.
    pub fn new() -> Self {
        Self
    }
}

/// Build the Bing RSS search URL for `query`, pinned to the English
/// interface (`setlang=en`) and English-US market (`mkt=en-US`) so localized
/// result feeds cannot leak into English queries.
fn rss_url(query: &str) -> String {
    let params: String = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("format", "rss")
        .append_pair("q", query)
        .append_pair("setlang", "en")
        .append_pair("mkt", "en-US")
        .finish();
    format!("https://www.bing.com/search?{params}")
}

impl SearchEngineBackend for BingBackend {
    fn name(&self) -> SearchEngine {
        SearchEngine::Bing
    }

    fn requires_browser(&self) -> bool {
        false
    }

    async fn search(
        &self,
        query: &str,
        count: usize,
    ) -> Result<Vec<EngineSearchResult>, SearchEngineError> {
        let engine = self.name();
        let url = rss_url(query);
        let response = crate::engine::http_client()
            .get(url.as_str())
            .send()
            .await
            .map_err(|e| SearchEngineError::Network {
                engine,
                detail: format!("request failed: {e}"),
            })?;

        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS
            || status == reqwest::StatusCode::FORBIDDEN
        {
            return Err(SearchEngineError::RateLimited {
                engine,
                detail: format!("HTTP {status}"),
            });
        }
        if !status.is_success() {
            return Err(SearchEngineError::Network {
                engine,
                detail: format!("HTTP {status}"),
            });
        }

        let body = response.text().await.map_err(|e| SearchEngineError::Network {
            engine,
            detail: format!("failed to read response body: {e}"),
        })?;

        let results = match parse_results(&body) {
            Ok(items) => items,
            // A block page (Turnstile/consent/challenge) masquerading as a
            // results response — report Captcha so the router applies a
            // cooldown.
            Err(e @ SearchEngineError::Parse { .. }) if body_has_block_markers(&body) => {
                return Err(SearchEngineError::Captcha {
                    engine,
                    detail: format!(
                        "Bing served a challenge/consent page instead of an RSS feed: {e}"
                    ),
                });
            }
            Err(e) => return Err(e),
        };
        let results: Vec<_> = results.into_iter().take(count).collect();

        tracing::debug!("bing: {query} -> {} results", results.len());
        Ok(results)
    }
}

/// Matches a full `<item>...</item>` block inside Bing's RSS channel.
static ITEM_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)<item\b[^>]*>(.*?)</item>").expect("valid bing item regex")
});

/// `<title>` field inside an item block.
static TITLE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)<title\b[^>]*>(.*?)</title>").expect("valid bing title regex")
});

/// `<link>` field inside an item block.
static LINK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)<link\b[^>]*>(.*?)</link>").expect("valid bing link regex")
});

/// `<description>` field inside an item block.
static DESC_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)<description\b[^>]*>(.*?)</description>").expect("valid bing description regex")
});

/// Parses a Bing RSS 2.0 response into normalized results with 1-based
/// positions.
///
/// A well-formed feed with zero `<item>` entries is a legitimate "no
/// results" response and yields an empty vector. Bodies without an
/// `<rss>`/`<channel>` root are not RSS at all and raise a
/// [`SearchEngineError::Parse`].
fn parse_results(body: &str) -> Result<Vec<EngineSearchResult>, SearchEngineError> {
    if !body.contains("<rss") && !body.contains("<channel") {
        return Err(SearchEngineError::Parse {
            engine: SearchEngine::Bing,
            detail: "response is not an RSS feed (no <rss>/<channel> root element)".to_string(),
        });
    }

    let mut results = Vec::new();
    for item in ITEM_RE.find_iter(body) {
        let block = item.as_str();
        let Some(title) = TITLE_RE
            .captures(block)
            .and_then(|c| c.get(1))
            .map(|m| clean_text(m.as_str()))
        else {
            continue;
        };
        let Some(url) = LINK_RE
            .captures(block)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().trim().to_string())
        else {
            continue;
        };
        let snippet = DESC_RE
            .captures(block)
            .and_then(|c| c.get(1))
            .map(|m| clean_text(m.as_str()))
            .unwrap_or_default();

        results.push(EngineSearchResult {
            title,
            url,
            snippet,
            position: results.len() + 1,
            engine: SearchEngine::Bing,
        });
    }
    Ok(results)
}

/// Decodes XML entities in `raw` (named and decimal/hex numeric), strips any
/// remaining markup tags, and collapses whitespace.
fn clean_text(raw: &str) -> String {
    collapse_whitespace(&strip_tags(&decode_entities(raw)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fixture mirroring the live `bing.com/search?format=rss` shape: two
    /// organic items with XML-escaped markup in titles/descriptions.
    const FIXTURE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:news="http://www.bing.com/news" xmlns:dc="http://purl.org/dc/elements/1.1/">
  <channel>
    <title>rust docs - Bing</title>
    <link>https://www.bing.com/search?format=rss&amp;q=rust+docs</link>
    <description>Search results for rust docs</description>
    <item>
      <title>Example Docs &amp; Guides</title>
      <link>https://example.com/docs/intro</link>
      <description>An intro to the docs with &lt;b&gt;highlighted&lt;/b&gt; terms &amp; more.</description>
    </item>
    <item>
      <title>Rust (programming language) - Wikipedia</title>
      <link>https://en.wikipedia.org/wiki/Rust_(programming_language)</link>
      <description>Rust is a systems programming language focused on safety &#39;and&#39; performance.</description>
    </item>
  </channel>
</rss>"#;

    #[test]
    fn parses_rss_items_with_titles_urls_snippets_and_positions() {
        let results = parse_results(FIXTURE).expect("fixture should parse");
        assert_eq!(results.len(), 2);

        assert_eq!(results[0].position, 1);
        assert_eq!(results[0].title, "Example Docs & Guides");
        assert_eq!(results[0].url, "https://example.com/docs/intro");
        assert_eq!(
            results[0].snippet,
            "An intro to the docs with highlighted terms & more."
        );
        assert_eq!(results[0].engine, SearchEngine::Bing);

        assert_eq!(results[1].position, 2);
        assert_eq!(results[1].title, "Rust (programming language) - Wikipedia");
        assert_eq!(
            results[1].url,
            "https://en.wikipedia.org/wiki/Rust_(programming_language)"
        );
        assert_eq!(
            results[1].snippet,
            "Rust is a systems programming language focused on safety 'and' performance."
        );
        assert_eq!(results[1].engine, SearchEngine::Bing);
    }

    #[test]
    fn empty_rss_feed_yields_no_results() {
        let body = r#"<?xml version="1.0"?><rss version="2.0"><channel>
            <title>no results - Bing</title>
            <link>https://www.bing.com/search?format=rss&amp;q=zzz</link>
            <description>No results found for zzz</description>
        </channel></rss>"#;
        let results = parse_results(body).expect("well-formed empty feed should parse");
        assert!(results.is_empty(), "zero items must yield an empty vector");
    }

    #[test]
    fn non_rss_body_is_parse_error() {
        let err = parse_results("<html><body><p>something else entirely</p></body></html>")
            .expect_err("non-RSS body must be a Parse error");
        assert!(matches!(err, SearchEngineError::Parse { .. }));
    }

    #[test]
    fn block_markers_detect_turnstile_consent_and_challenge() {
        assert!(body_has_block_markers(
            "<html><div id=\"cf-turnstile\"></div></html>"
        ));
        assert!(body_has_block_markers("cf-chl-1 challenge payload"));
        assert!(body_has_block_markers(
            "<html><form action=\"/consent\">Choose your cookies</form></html>"
        ));
        assert!(body_has_block_markers("verifying you are not a bot... challenge"));
        assert!(body_has_block_markers("captcha required"));
        assert!(!body_has_block_markers(FIXTURE));
        assert!(!body_has_block_markers("<html><body>plain page</body></html>"));
    }

    #[test]
    fn decodes_entities_and_strips_tags() {
        assert_eq!(clean_text("A &amp; B &lt;3"), "A & B <3");
        assert_eq!(
            clean_text("with &lt;b&gt;bold&lt;/b&gt; &amp; &#39;quoted&#39; &#x27;terms&#x27;"),
            "with bold & 'quoted' 'terms'"
        );
        assert_eq!(clean_text("  spaced\n\t out  "), "spaced out");
        // Unknown/malformed entities survive verbatim.
        assert_eq!(clean_text("a &unknown; &"), "a &unknown; &");
    }

    #[test]
    fn backend_metadata() {
        let backend = BingBackend::new();
        assert_eq!(backend.name(), SearchEngine::Bing);
        assert!(!backend.requires_browser());
        assert_eq!(SearchEngine::Bing.as_str(), "bing");
    }

    #[test]
    fn rss_url_includes_english_language_and_market_params() {
        let url = rss_url("rust docs");
        assert!(url.starts_with("https://www.bing.com/search?"));
        assert!(url.contains("format=rss"), "existing params preserved");
        assert!(url.contains("q=rust+docs"), "query must be form-encoded");
        assert!(url.contains("setlang=en"), "English language param must be present");
        assert!(url.contains("mkt=en-US"), "English-US market param must be present");
    }
}
