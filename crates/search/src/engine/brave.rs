//! Brave search backend (plain HTTP, no browser).
//!
//! Fetches `https://search.brave.com/search?q=...&source=web&hl=en` and
//! parses the server-rendered (SSR) result page. Brave serves organic results
//! as
//! `<div class="snippet ..." data-pos="N" data-type="web" data-keynav="true">`
//! blocks (live-verified 2026-08-03: 200 with ~20 `data-type="web"` blocks,
//! no CAPTCHA). The `svelte-XXXX` scoped class hashes are per-build and
//! unstable, so parsing relies exclusively on the stable semantic markers:
//!
//! * block: any `<div ... data-type="web" ...>` opening tag (cluster/video
//!   blocks carry other `data-type` values and are skipped)
//! * title: `div.title.search-snippet-title.line-clamp-1` (text content)
//! * url: the `href` of the first `<a>` in the block — the title anchor
//!   (direct, absolute URL — no redirect wrapper)
//! * snippet: `div.generic-snippet > div.content` (text content)
//!
//! The live endpoint sets no cookies of its own but accepts the ddgs-style
//! manual cookie set (`country`/`useLocation`/`safesearch`) — verified 200
//! with the same block shape when the Cookie header is sent.

use regex::Regex;
use reqwest::header::{HeaderMap, HeaderValue, COOKIE};

use super::html::{body_has_block_markers, collapse_whitespace, decode_entities, strip_tags};
use super::{EngineSearchResult, SearchEngine, SearchEngineBackend, SearchEngineError};

/// Manual cookie set matching the community (ddgs, PR #397) recipe. The live
/// server sends no `Set-Cookie` for this endpoint, but serves a normal 200
/// with ~20 `data-type="web"` blocks when this header is present.
const COOKIE_VALUE: &str = "country=us; useLocation=0; safesearch=moderate";

/// Per-request headers: the shared client already sends the Chrome
/// User-Agent and `Accept-Language`; the manual Cookie header is added here.
fn request_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(COOKIE, HeaderValue::from_static(COOKIE_VALUE));
    headers
}

/// Stateless Brave backend.
pub struct BraveBackend;

/// Build the Brave SERP URL for `query`, pinned to the English interface
/// (`hl=en`) so localized result pages cannot leak into English queries.
fn serp_url(query: &str) -> String {
    format!(
        "https://search.brave.com/search?q={}&source=web&hl=en",
        url::form_urlencoded::byte_serialize(query.as_bytes()).collect::<String>()
    )
}

impl SearchEngineBackend for BraveBackend {
    fn name(&self) -> SearchEngine {
        SearchEngine::Brave
    }

    fn requires_browser(&self) -> bool {
        false
    }

    async fn search(
        &self,
        query: &str,
        count: usize,
    ) -> Result<Vec<EngineSearchResult>, SearchEngineError> {
        // No `count` query parameter: live probes show the server ignores it
        // (still ~20 blocks) and ddgs does not send it; `count` is enforced
        // client-side via `parse_results`.
        let url = serp_url(query);

        let resp = crate::engine::http_client()
            .get(&url)
            .headers(request_headers())
            .send()
            .await
            .map_err(|e| SearchEngineError::Network {
                engine: SearchEngine::Brave,
                detail: format!("request failed: {e}"),
            })?;

        let status = resp.status();
        if status == reqwest::StatusCode::FORBIDDEN
            || status == reqwest::StatusCode::TOO_MANY_REQUESTS
        {
            return Err(SearchEngineError::RateLimited {
                engine: SearchEngine::Brave,
                detail: format!("HTTP {status}"),
            });
        }
        if !status.is_success() {
            return Err(SearchEngineError::Network {
                engine: SearchEngine::Brave,
                detail: format!("unexpected HTTP {status}"),
            });
        }

        let html = resp.text().await.map_err(|e| {
            SearchEngineError::Network {
                engine: SearchEngine::Brave,
                detail: format!("failed to read response body: {e}"),
            }
        })?;

        let results = parse_results(&html, count)?;
        tracing::debug!("brave: {query} -> {} results", results.len());
        Ok(results)
    }
}

/// Matches a result block opening tag: any `<div ... data-type="web" ...>`.
/// Attribute order is free — the live server emits `class` before
/// `data-type`, but the marker is what matters, not the position.
fn block_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"<div\b[^>]*data-type="web"[^>]*>"#).expect("invalid block regex")
    })
}

/// Extracts the result `(url, title, snippet)` from a single block's inner
/// HTML (everything after the opening `<div ... data-type="web" ...>`).
///
/// Live markup inside each block (verified 2026-08-03):
/// `<div class="result-wrapper ..."><div class="result-content ...">`
/// `<a href="URL" target="_self" class="... l1">` — title anchor wrapping a
/// `div.site-name-wrapper` (favicon + cite) followed by
/// `<div class="title search-snippet-title line-clamp-1 ...">Title</div>`,
/// then `</a>` and `<div class="generic-snippet ..."><div
/// class="content ...">Snippet</div></div>`. Svelte `<!---->` comment
/// markers are interleaved throughout; they are inert to the regexes.
fn block_re_inner() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"(?s)<a\b[^>]*href="([^"]*)"[^>]*>.*?<div\b[^>]*class="[^"]*\btitle\b[^"]*\bsearch-snippet-title\b[^"]*"[^>]*>(.*?)</div>.*?<div\b[^>]*class="[^"]*\bgeneric-snippet\b[^"]*"[^>]*>\s*<div\b[^>]*class="[^"]*\bcontent\b[^"]*"[^>]*>(.*?)</div>"#,
        )
        .expect("invalid block inner regex")
    })
}

/// Strips a leading relative-date prefix from a Brave snippet, e.g.
/// `1 week ago -Rust began...` → `Rust began...`. Brave prepends a
/// `t-secondary` span like `1 week ago -` (no space after the dash) to some
/// snippets; the prefix is metadata, not result text, and must not leak into
/// the snippet. The rest of the snippet is preserved intact.
fn strip_date_prefix(snippet: &str) -> String {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"^\d+\s+(day|week|month|year)s?\s+ago\s*-?\s*").expect("invalid date regex")
    });
    re.replace(snippet, "").into_owned()
}

/// Parse Brave SSR result blocks from `html`, returning up to `count`
/// results with 1-based page positions. Non-web blocks (`data-type`
/// values other than `"web"`, e.g. `cluster`) and blocks without a full
/// title/url/snippet triple are skipped.
///
/// A body with no result blocks that carries block-page markers (CAPTCHA /
/// challenge) is reported as [`SearchEngineError::Captcha`] so the router can
/// apply a cooldown.
pub(crate) fn parse_results(
    html: &str,
    count: usize,
) -> Result<Vec<EngineSearchResult>, SearchEngineError> {
    let blocks = extract_blocks(html);
    if blocks.is_empty() && body_has_block_markers(html) {
        return Err(SearchEngineError::Captcha {
            engine: SearchEngine::Brave,
            detail: "Brave served a challenge/CAPTCHA page instead of results".to_string(),
        });
    }

    let mut results = Vec::new();
    let mut matched = 0usize;

    for (position, block) in blocks.into_iter().enumerate() {
        if matched >= count {
            break;
        }
        let Some(caps) = block_re_inner().captures(&block) else {
            continue;
        };
        let url = caps.get(1).map(|m| m.as_str().trim().to_string());
        let title = caps
            .get(2)
            .map(|m| collapse_whitespace(&decode_entities(&strip_tags(m.as_str()))));
        let snippet = caps
            .get(3)
            .map(|m| strip_date_prefix(&collapse_whitespace(&decode_entities(&strip_tags(m.as_str())))));

        match (title, url, snippet) {
            (Some(title), Some(url), Some(snippet))
                if !title.is_empty() && !url.is_empty() && !snippet.is_empty() =>
            {
                results.push(EngineSearchResult {
                    title,
                    url,
                    snippet,
                    position: position + 1,
                    engine: SearchEngine::Brave,
                });
                matched += 1;
            }
            _ => {}
        }
    }
    Ok(results)
}

/// Find all `div[data-type='web']` blocks (including their inner HTML) by
/// tracking div depth from the opening tag.
///
/// Both `<div` openings and `</div>` closings are counted, so multi-line
/// blocks with nested divs are captured in full — a block must never be
/// truncated at its first inner `</div>` (which is what breaks the inner
/// regex on server-rendered, multi-line result blocks).
fn extract_blocks(html: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut idx = 0usize;
    while let Some(m) = block_re().find_at(html, idx) {
        let start = m.start();
        let mut depth = 1usize;
        let mut i = m.end();
        // Scan for balanced `<div` / `</div>` pairs to bound the block.
        while i < html.len() {
            let rest = &html[i..];
            let open = rest.find("<div");
            let close = rest.find("</div");
            let next = match (open, close) {
                (Some(o), Some(c)) => o.min(c),
                (Some(o), None) => o,
                (None, Some(c)) => c,
                (None, None) => break,
            };
            if rest.as_bytes()[next + 1] == b'/' {
                // Closing tag. `</div>` normally, but Brave may also emit
                // `</div ` without the `>`; accept both.
                let after = rest[next + 5..].chars().next();
                if after == Some('>') || after == Some(' ') {
                    depth -= 1;
                    if depth == 0 {
                        let end = 5
                            + rest[next + 5..]
                                .find('>')
                                .map(|p| p + 1)
                                .unwrap_or(0);
                        blocks.push(html[start..i + next + end].to_string());
                        idx = i + next + end;
                        break;
                    }
                }
                i += next + 5;
            } else {
                // Opening tag; only counts when it starts a real tag.
                let after = rest[next + 4..].chars().next();
                if matches!(
                    after,
                    Some('>') | Some(' ') | Some('\n') | Some('\t') | Some('\r') | None
                ) {
                    depth += 1;
                }
                i += next + 4;
            }
        }
        if depth != 0 {
            break; // unbalanced remainder; stop scanning
        }
    }
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fixture captured live from `https://search.brave.com/search?q=rust
    /// +lang&source=web` on 2026-08-03 (blocks minified only): two
    /// `data-type="web"` results (rust-lang.org, Wikipedia with deep links)
    /// followed by a `data-type="cluster"` Videos block that must be
    /// ignored. Svelte comment markers, scoped class hashes, favicon srcs,
    /// and entity-encoded text (`&nbsp;`) are preserved verbatim.
    const FIXTURE: &str = r#"<!DOCTYPE html>
<html lang="en">
<head><title>brave search</title></head>
<body>
  <div class="results" id="results">
<div class="snippet svelte-jmfu5f" data-pos="1" data-type="web" data-keynav="true"><div class="result-wrapper svelte-1rq4ngz"><div class="result-content svelte-1rq4ngz"><a href="https://rust-lang.org/en-US/" target="_self" class="svelte-14r20fy l1"><div class="site-name-wrapper svelte-on1hvy"><div class="favicon-wrapper svelte-on1hvy"><!--[0--><div class="favicon-background-wrapper svelte-on1hvy"><img class="favicon-background svelte-on1hvy" src="https://imgs.search.brave.com/KM3UVnh_2VxQjDofcZZ7qQzMlQVdYlUJC5n4bboPPGU/rs:fit:32:32:1:0/g:ce/aHR0cDovL2Zhdmlj/b25zLnNlYXJjaC5i/cmF2ZS5jb20vaWNv/bnMvN2MzNTM1MGEx/ZTA0YTg3Y2U4NjA0/MTc1N2ViYjlkZDg5/OGY3NGQzMTliZGM2/Nzc1ZWMwMDlkNjhl/NTg1OGVlMC9ydXN0/LWxhbmcub3JnLw" alt="" loading="lazy" decoding="async" aria-hidden="true"/></div><!--]--><img alt="🌐" class="favicon svelte-w2a9kc size-s" src="https://imgs.search.brave.com/KM3UVnh_2VxQjDofcZZ7qQzMlQVdYlUJC5n4bboPPGU/rs:fit:32:32:1:0/g:ce/aHR0cDovL2Zhdmlj/b25zLnNlYXJjaC5i/cmF2ZS5jb20vaWNv/bnMvN2MzNTM1MGEx/ZTA0YTg3Y2U4NjA0/MTc1N2ViYjlkZDg5/OGY3NGQzMTliZGM2/Nzc1ZWMwMDlkNjhl/NTg1OGVlMC9ydXN0/LWxhbmcub3JnLw" loading="lazy" onerror="this.__e=event"/><!----><!--[-1--><!--]--></div><div class="site-name-content svelte-on1hvy"><div class="desktop-small-semibold t-secondary text-ellipsis">Rust</div><div class="url-wrapper svelte-on1hvy"><cite class="snippet-url desktop-small-regular t-tertiary svelte-on1hvy">rust-lang.org <span class="text-ellipsis">› en-US</span></cite><!--[-1--><!--]--></div></div><!--[-1--><!--]--></div><!----><div class="title search-snippet-title line-clamp-1 svelte-14r20fy" title="Rust Programming Language">Rust Programming Language</div></a><!----><!--[0--><div class="generic-snippet svelte-1cwdgg3"><div class="content desktop-default-regular t-primary line-clamp-dynamic svelte-1cwdgg3"><!--[-1--><!--]--><!---->Redirecting to /<!----></div><!--[-1--><!--]--></div><!--]--><!--[0--><!--[-1--><!--]--><!--]--></div><!--[-1--><!--]--></div><!--[-1--><!--]--><!----><!--[-1--><!--]--><!--[-1--><!--]--><!--[-1--><!--]--><!----><!--[-1--><!--]--></div><!--]--><!--]--><!--[5--><!--[0-->
<div class="snippet svelte-jmfu5f" data-pos="2" data-type="web" data-keynav="true"><div class="result-wrapper svelte-1rq4ngz"><div class="result-content svelte-1rq4ngz"><a href="https://en.wikipedia.org/wiki/Rust_(programming_language)" target="_self" class="svelte-14r20fy l1"><div class="site-name-wrapper svelte-on1hvy"><div class="favicon-wrapper svelte-on1hvy"><!--[0--><div class="favicon-background-wrapper svelte-on1hvy"><img class="favicon-background svelte-on1hvy" src="https://imgs.search.brave.com/m6XxME4ek8DGIUcEPCqjRoDjf2e54EwL9pQzyzogLYk/rs:fit:32:32:1:0/g:ce/aHR0cDovL2Zhdmlj/b25zLnNlYXJjaC5i/cmF2ZS5jb20vaWNv/bnMvNjQwNGZhZWY0/ZTQ1YWUzYzQ3MDUw/MmMzMGY3NTQ0ZjNj/NDUwMDk5ZTI3MWRk/NWYyNTM4N2UwOTE0/NTI3ZDQzNy9lbi53/aWtpcGVkaWEub3Jn/Lw" alt="" loading="lazy" decoding="async" aria-hidden="true"/></div><!--]--><img alt="🌐" class="favicon svelte-w2a9kc size-s" src="https://imgs.search.brave.com/m6XxME4ek8DGIUcEPCqjRoDjf2e54EwL9pQzyzogLYk/rs:fit:32:32:1:0/g:ce/aHR0cDovL2Zhdmlj/b25zLnNlYXJjaC5i/cmF2ZS5jb20vaWNv/bnMvNjQwNGZhZWY0/ZTQ1YWUzYzQ3MDUw/MmMzMGY3NTQ0ZjNj/NDUwMDk5ZTI3MWRk/NWYyNTM4N2UwOTE0/NTI3ZDQzNy9lbi53/aWtpcGVkaWEub3Jn/Lw" loading="lazy" onerror="this.__e=event"/><!----><!--[-1--><!--]--></div><div class="site-name-content svelte-on1hvy"><div class="desktop-small-semibold t-secondary text-ellipsis">Wikipedia</div><div class="url-wrapper svelte-on1hvy"><cite class="snippet-url desktop-small-regular t-tertiary svelte-on1hvy">en.wikipedia.org <span class="text-ellipsis">› wiki  › Rust_(programming_language)</span></cite><!--[-1--><!--]--></div></div><!--[-1--><!--]--></div><!----><div class="title search-snippet-title line-clamp-1 svelte-14r20fy" title="Rust (programming language) - Wikipedia">Rust (programming language) - Wikipedia</div></a><!----><!--[0--><div class="generic-snippet svelte-1cwdgg3"><div class="content desktop-default-regular t-primary line-clamp-dynamic svelte-1cwdgg3"><!--[0--><span class="t-secondary">1 week ago -</span><!--]--><!----><strong>Rust</strong> began as a personal project by Mozilla employee Graydon Hoare in 2006. According to MIT Technology Review, he started the project due to his frustration with a broken elevator in his apartment building whose software had crashed, and named the language after the group of fungi of the same&nbsp;...<!----></div><!--[-1--><!--]--></div><!--]--><!--[0--><!--[-1--><!--]--><!--]--></div><!--[-1--><!--]--></div><!--[0--><!--[-1--><div class="deep-links svelte-3l1gt9" style="margin-top: 0px;"><!--[--><a class="deep-link components-button-small t-interactive svelte-3l1gt9" href="https://en.wikipedia.org/wiki/Rust_(programming_language)#History" target="_self"><span class="svelte-3l1gt9">History</span></a><a class="deep-link components-button-small t-interactive svelte-3l1gt9" href="https://en.wikipedia.org/wiki/Rust_(programming_language)#Syntax_and_features" target="_self"><span class="svelte-3l1gt9">Syntax and features</span></a><a class="deep-link components-button-small t-interactive svelte-3l1gt9" href="https://en.wikipedia.org/wiki/Rust_(programming_language)#Safety" target="_self"><span class="svelte-3l1gt9">Safety</span></a><a class="deep-link components-button-small t-interactive svelte-3l1gt9" href="https://en.wikipedia.org/wiki/Rust_(programming_language)#Ecosystem" target="_self"><span class="svelte-3l1gt9">Ecosystem</span></a><a class="deep-link components-button-small t-interactive svelte-3l1gt9" href="https://en.wikipedia.org/wiki/Rust_(programming_language)#Performance" target="_self"><span class="svelte-3l1gt9">Performance</span></a><a class="deep-link components-button-small t-interactive svelte-3l1gt9" href="https://en.wikipedia.org/wiki/Rust_(programming_language)#Adoption" target="_self"><span class="svelte-3l1gt9">Adoption</span></a><a class="deep-link components-button-small t-interactive svelte-3l1gt9" href="https://en.wikipedia.org/wiki/Rust_(programming_language)#In_academic_research" target="_self"><span class="svelte-3l1gt9">In academic research</span></a><a class="deep-link components-button-small t-interactive svelte-3l1gt9" href="https://en.wikipedia.org/wiki/Rust_(programming_language)#Community" target="_self"><span class="svelte-3l1gt9">Community</span></a><!--]--><!--[-1--><!--]--><!--[-1--><!--]--></div><!--]--><!--]--><!----><!--[-1--><!--]--><!--[-1--><!--]--><!--[-1--><!--]--><!----><!--[-1--><!--]--></div><!--]--><!--]--><!--[3--><!--[0--><!--[0-->
<div class="snippet standalone svelte-jmfu5f" data-pos="3" data-type="cluster" data-keynav="true"><header class="mb-xl svelte-1fzoz3n"><span class="desktop-heading-h4 t-secondary">Videos</span></header><a tabindex="0" class="enrichment-card-item svelte-kobgr0" href="https://www.youtube.com/watch?v=fTXtdbt1PFA" target="_blank" rel="noopener"><div class="enrichment-card-duration desktop-xsmall-semibold svelte-kobgr0">30:30</div></a></div>
  </div>
</body>
</html>"#;

    #[test]
    fn parses_web_result_blocks() {
        let results = parse_results(FIXTURE, 10).expect("fixture should parse");
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Rust Programming Language");
        assert_eq!(results[0].url, "https://rust-lang.org/en-US/");
        assert_eq!(results[0].snippet, "Redirecting to /");
        assert_eq!(results[0].position, 1);
        assert_eq!(results[0].engine, SearchEngine::Brave);
    }

    #[test]
    fn parses_wikipedia_block_with_deep_links() {
        let results = parse_results(FIXTURE, 10).expect("fixture should parse");
        assert_eq!(results[1].title, "Rust (programming language) - Wikipedia");
        assert_eq!(
            results[1].url,
            "https://en.wikipedia.org/wiki/Rust_(programming_language)"
        );
        // The deep-links section inside the block must not leak into the
        // snippet, the snippet text is entity-decoded (`&nbsp;`), and the
        // leading relative-date prefix (`1 week ago -`) is stripped.
        assert!(
            results[1]
                .snippet
                .starts_with("Rust began as a personal project")
        );
        assert!(results[1].snippet.ends_with("of the same ..."));
        assert!(!results[1].snippet.contains("History"));
        assert_eq!(results[1].position, 2);
    }

    #[test]
    fn respects_count_limit() {
        let results = parse_results(FIXTURE, 1).expect("fixture should parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "https://rust-lang.org/en-US/");
        assert_eq!(results[0].position, 1);
    }

    #[test]
    fn ignores_non_web_blocks() {
        // The Videos cluster (data-type="cluster") carries an anchor but must
        // be skipped; only the two data-type="web" blocks parse.
        let results = parse_results(FIXTURE, 10).expect("fixture should parse");
        assert!(
            results.iter().all(|r| !r.url.contains("youtube.com")),
            "cluster block must never surface"
        );
    }

    #[test]
    fn ignores_web_blocks_missing_parts() {
        // A data-type="web" block without a generic-snippet is dropped
        // entirely (all three fields must be present).
        let html = r#"<html><body>
          <div class="snippet svelte-1" data-type="web">
            <a href="https://full.example/"><div class="title search-snippet-title line-clamp-1 svelte-1">Full Result</div></a>
            <div class="generic-snippet svelte-1"><div class="content svelte-1">Has everything.</div></div>
          </div>
          <div class="snippet svelte-2" data-type="web">
            <a href="https://partial.example/"><div class="title search-snippet-title line-clamp-1 svelte-2">No Snippet</div></a>
          </div>
        </body></html>"#;
        let results = parse_results(html, 10).expect("fixture should parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Full Result");
    }

    #[test]
    fn collects_results_after_non_matching_blocks() {
        // A data-type="web" block that fails to parse (no title) must not
        // consume the count slot; the valid block after it is still returned.
        let html = r#"<html><body>
          <div class="snippet svelte-1" data-type="web">
            <a href="https://broken.example/">broken block</a>
          </div>
          <div class="snippet svelte-2" data-type="web">
            <a href="https://good.example/"><div class="title search-snippet-title line-clamp-1 svelte-2">Good Result</div></a>
            <div class="generic-snippet svelte-2"><div class="content svelte-2">Has everything.</div></div>
          </div>
        </body></html>"#;
        let results = parse_results(html, 1).expect("fixture should parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Good Result");
        assert_eq!(results[0].url, "https://good.example/");
    }

    #[test]
    fn empty_on_no_blocks() {
        let results =
            parse_results("<html><body><p>no results here</p></body></html>", 10)
                .expect("plain body should parse to empty");
        assert!(results.is_empty());
    }

    #[test]
    fn captcha_body_without_blocks_is_captcha_error() {
        let err = parse_results(
            "<html><body><div id=\"cf-turnstile\">Verify you are human</div></body></html>",
            10,
        )
        .expect_err("block page must be a Captcha error");
        assert!(matches!(err, SearchEngineError::Captcha { .. }));
    }

    #[test]
    fn strips_html_entities_in_text() {
        assert_eq!(
            collapse_whitespace(&decode_entities(&strip_tags("C++ &amp; Rust &lt;3"))),
            "C++ & Rust <3"
        );
    }

    #[test]
    fn strips_relative_date_prefix_from_snippet() {
        assert_eq!(
            strip_date_prefix("1 week ago -Rust began as a personal project"),
            "Rust began as a personal project"
        );
        assert_eq!(
            strip_date_prefix("2 days ago -Some text"),
            "Some text"
        );
        assert_eq!(
            strip_date_prefix("3 months ago-No space after dash"),
            "No space after dash"
        );
        // No prefix → unchanged.
        assert_eq!(
            strip_date_prefix("Rust began as a personal project"),
            "Rust began as a personal project"
        );
    }

    #[test]
    fn cookie_header_matches_community_recipe() {
        let headers = request_headers();
        let cookie = headers
            .get(COOKIE)
            .and_then(|v| v.to_str().ok())
            .expect("cookie header must be present");
        assert_eq!(cookie, "country=us; useLocation=0; safesearch=moderate");
    }

    #[test]
    fn serp_url_includes_english_language_hint() {
        let url = serp_url("rust lang");
        assert!(url.starts_with("https://search.brave.com/search?"));
        assert!(url.contains("q=rust+lang"), "query must be form-encoded");
        assert!(url.contains("source=web"), "existing params preserved");
        assert!(url.contains("hl=en"), "English language hint must be present");
    }
}
