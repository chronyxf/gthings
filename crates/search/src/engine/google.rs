//! Google search backend via CDP.
//!
//! Mirrors the original Google search flow in [`crate::search`]: navigates
//! to the SERP with `q`/`num`/`hl` params, detects CAPTCHA/access-denied
//! pages, scrolls to trigger lazy loading, extracts organic results with
//! the shared `search_extract.js` template, and post-processes them
//! (junk filter, `#:~:text=` strip, empty-snippet drop, base-URL dedup,
//! title/snippet cleaning, 1-based position renumbering).
//!
//! The backend owns its tab: each search creates a background tab, and the
//! tab is *always* closed afterwards (close failures are logged, not
//! propagated). CDP errors are mapped onto [`SearchEngineError`]. Unlike the
//! legacy `crate::search` implementation, provenance and domain-authority
//! construction are omitted — the orchestrator computes those later.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock};

use gthings_cdp::{CdpError, Session, Tab};
use gthings_common::safe_truncate_end;
use serde::Deserialize;

use super::{EngineSearchResult, SearchEngine, SearchEngineBackend, SearchEngineError};

/// Google search backend driving a real browser via CDP.
pub struct GoogleBackend {
    session: Arc<Session>,
}

impl GoogleBackend {
    /// Create a backend bound to the given browser session.
    pub fn new(session: Arc<Session>) -> Self {
        Self { session }
    }

    /// Single search attempt (no retry) inside a freshly created background
    /// tab. The tab is always closed before returning. `scroll` controls
    /// whether the conditional lazy-load scroll runs (the retry attempt
    /// passes `false` to avoid a redundant second scroll).
    async fn search_once(
        &self,
        query: &str,
        count: usize,
        scroll: bool,
    ) -> Result<Vec<EngineSearchResult>, SearchEngineError> {
        let tab = self
            .session
            .create_background_tab()
            .await
            .map_err(map_cdp_error)?;
        let outcome = self.search_in_tab(&tab, query, count, scroll).await;
        if let Err(e) = self.session.close_tab(tab).await {
            tracing::warn!("google: failed to close background tab: {e}");
        }
        outcome
    }

    /// Navigate `tab` to the Google SERP and extract results. Mirrors the
    /// original `search_once` flow, minus provenance/domain-authority
    /// construction (the router computes those later).
    #[allow(clippy::incompatible_msrv)]
    async fn search_in_tab(
        &self,
        tab: &Tab,
        query: &str,
        count: usize,
        scroll: bool,
    ) -> Result<Vec<EngineSearchResult>, SearchEngineError> {
        let params: String = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("q", query)
            .append_pair("num", &(count * 2).clamp(10, 100).to_string())
            .append_pair("hl", "en")
            .finish();
        let url = format!("https://www.google.com/search?{params}");

        tab.navigate(&self.session, &url)
            .await
            .map_err(map_cdp_error)?;

        // Check for Google CAPTCHA/Sorry block — URL and title in a single
        // CDP evaluate to save a round-trip.
        let page_info = tab
            .evaluate(
                &self.session,
                "JSON.stringify({url: window.location.href, title: document.title})",
            )
            .await
            .map_err(map_cdp_error)?;
        let info_str = page_info
            .get("result")
            .and_then(|r| r.get("value"))
            .and_then(|v| v.as_str())
            .unwrap_or("{}");
        let info: serde_json::Value = serde_json::from_str(info_str).unwrap_or_default();
        let current_url = info.get("url").and_then(|v| v.as_str()).unwrap_or("");
        let title = info.get("title").and_then(|v| v.as_str()).unwrap_or("");
        if is_captcha_url(current_url) {
            tracing::warn!("Google CAPTCHA/Sorry page detected at: {current_url}");
            return Err(SearchEngineError::Captcha {
                engine: SearchEngine::Google,
                detail: format!(
                    "Google served CAPTCHA page instead of search results: {current_url}"
                ),
            });
        }
        if is_captcha_title(title) {
            tracing::warn!("Google access-denied page detected: {title}");
            return Err(SearchEngineError::Captcha {
                engine: SearchEngine::Google,
                detail: format!(
                    "Google returned access-denied page '{title}' instead of search results"
                ),
            });
        }

        // In-browser JS: iterate all links, skip self-hosted, extract snippet
        // via attribute-based selectors (no brittle class-name dependency).
        let js = extraction_js(count);

        let mut items = extract_results(tab, &self.session, &js).await?;

        // Scroll down to trigger lazy loading of more organic results — but
        // only when the first extraction didn't already return enough. When
        // enough results are present the scroll is redundant and skipped
        // entirely. A single CDP evaluate runs the whole scroll sequence in
        // JS (bounded loop with a short async delay), so there are no
        // Rust-side sleeps.
        if scroll && should_scroll(items.len(), count) {
            let scroll_iterations = count.max(3);
            let target_count = count * 2;
            // Scroll in a bounded loop, but after each scroll poll the DOM
            // for the organic-result count and only stop once it stops
            // growing (3 consecutive stable polls) or reaches the target.
            // This waits out lazy-loading that arrives after a longer delay
            // instead of re-extracting immediately.
            let scroll_js = format!(
                "(async () => {{ const iters = {scroll_iterations}; const target = {target_count}; \
                 let last = 0, stable = 0; \
                 for (let i = 0; i < iters; i++) {{ \
                   window.scrollBy(0, 800); \
                   await new Promise(r => setTimeout(r, 150)); \
                   const n = document.querySelectorAll('div[data-hveid], div[data-sokoban-container]').length; \
                   if (n === last) {{ stable++; }} else {{ stable = 0; last = n; }} \
                   if (stable >= 3 || n >= target) break; \
                 }} return true; }})()"
            );
            tab.evaluate(&self.session, &scroll_js)
                .await
                .map_err(map_cdp_error)?;
            items = extract_results(tab, &self.session, &js).await?;
        }

        post_process_results(&mut items);

        Ok(items)
    }
}

/// Run the extraction JS in `tab` and parse the returned organic results.
#[allow(clippy::incompatible_msrv)]
async fn extract_results(
    tab: &Tab,
    session: &Session,
    js: &str,
) -> Result<Vec<EngineSearchResult>, SearchEngineError> {
    let result = tab.evaluate(session, js).await.map_err(map_cdp_error)?;
    let raw = result["result"]["value"].as_str();
    let json_str = raw.unwrap_or("[]");
    let raw_items: Vec<RawResult> = serde_json::from_str(json_str).map_err(|e| {
        let preview = &json_str[..json_str.floor_char_boundary(json_str.len().min(200))];
        tracing::warn!("google: failed to parse results JSON: {e} (preview: {preview:?})");
        SearchEngineError::Parse {
            engine: SearchEngine::Google,
            detail: e.to_string(),
        }
    })?;
    Ok(raw_items
        .into_iter()
        .map(|r| EngineSearchResult {
            title: r.title,
            url: r.url,
            snippet: r.snippet,
            position: r.position,
            engine: SearchEngine::Google,
        })
        .collect())
}

/// True when the first extraction returned fewer than `count * 2` results,
/// meaning a scroll may load more. Matches the `search_extract.js` break
/// threshold so we never scroll when the page already has enough results.
fn should_scroll(result_count: usize, count: usize) -> bool {
    result_count < count * 2
}

/// True when an empty result set warrants a single trailing-space retry.
/// Gated so we never retry when the query already ends with a space (the
/// trailing-space variant can't help) or when results were found.
fn should_retry(results_empty: bool, query: &str) -> bool {
    results_empty && !query.ends_with(' ')
}

impl SearchEngineBackend for GoogleBackend {
    fn name(&self) -> SearchEngine {
        SearchEngine::Google
    }

    fn requires_browser(&self) -> bool {
        true
    }

    async fn search(
        &self,
        query: &str,
        count: usize,
    ) -> Result<Vec<EngineSearchResult>, SearchEngineError> {
        let results = self.search_once(query, count, true).await?;
        let final_results = if should_retry(results.is_empty(), query) {
            // Retry ONCE with trailing space — Google sometimes returns zero
            // results for bare queries that work with a trailing space. The
            // retry skips the scroll (same page shape) so it avoids a second
            // full CDP scroll cost.
            let spaced = format!("{query} ");
            self.search_once(&spaced, count, false).await?
        } else {
            results
        };
        tracing::debug!("google: {query} -> {} results", final_results.len());
        Ok(final_results)
    }
}

/// Raw result shape produced by `search_extract.js` (title/url/snippet/
/// position; the `engine` field is filled in by the backend).
#[derive(Debug, Deserialize)]
struct RawResult {
    title: String,
    url: String,
    snippet: String,
    position: usize,
}

/// `search_extract.js` read from disk exactly once.
static TEMPLATE: OnceLock<String> = OnceLock::new();

/// Last `__COUNT__`-substituted template (template + the count it was built
/// for), so the `.replace()` only re-runs when the requested count changes.
static SUBSTITUTED_TEMPLATE: Mutex<Option<(usize, String)>> = Mutex::new(None);

/// Return the extraction JS with `__COUNT__` substituted for `count`. The
/// template is embedded once; the substitution is cached per count.
fn extraction_js(count: usize) -> String {
    let raw = TEMPLATE.get_or_init(|| include_str!("../../templates/search_extract.js").to_string());
    let mut cached = SUBSTITUTED_TEMPLATE.lock().unwrap();
    if let Some((c, js)) = cached.as_ref() {
        if *c == count {
            return js.clone();
        }
    }
    let js = raw.replace("__COUNT__", &count.to_string());
    *cached = Some((count, js.clone()));
    js
}

/// True when `url` looks like Google's CAPTCHA/Sorry block page.
fn is_captcha_url(url: &str) -> bool {
    url.contains("/sorry/") || url.contains("google.com/sorry")
}

/// True when `title` looks like Google's access-denied ("Accessibility
/// help" / "Learn more") block page.
fn is_captcha_title(title: &str) -> bool {
    title.contains("Accessibility") || title.contains("Learn more")
}

/// Map CDP errors onto [`SearchEngineError`]: CAPTCHA → `Captcha`, JSON →
/// `Parse`, everything else (navigation timeouts included) → `Unavailable`.
fn map_cdp_error(err: CdpError) -> SearchEngineError {
    match err {
        CdpError::CaptchaBlocked { detail } => SearchEngineError::Captcha {
            engine: SearchEngine::Google,
            detail,
        },
        CdpError::Json(e) => SearchEngineError::Parse {
            engine: SearchEngine::Google,
            detail: e.to_string(),
        },
        other => SearchEngineError::Unavailable {
            engine: SearchEngine::Google,
            detail: other.to_string(),
        },
    }
}

/// True when `suffix` looks like a domain token appended to a title, i.e.
/// it ends with a known TLD (`.com`, `.org`, `.net`, ...). Only such
/// suffixes are stripped from titles; arbitrary uppercase words are kept.
fn is_domain_suffix(suffix: &str) -> bool {
    const TLDS: &[&str] = &[
        "com", "org", "net", "io", "edu", "gov", "co", "uk", "us", "ca", "de", "fr", "jp",
        "au", "info", "biz", "me", "tv", "xyz", "dev", "app", "ai", "ru", "cn", "in", "br",
        "it", "es", "nl", "se", "pl", "ch", "at", "be", "dk", "fi", "no", "pt", "gr", "cz",
        "hu", "ro", "ua", "tr", "il", "kr", "tw", "hk", "sg", "my", "th", "vn", "ph", "id",
        "nz", "za", "mx", "ar", "cl", "pe", "ve",
    ];
    let lower = suffix.to_ascii_lowercase();
    TLDS.iter().any(|tld| lower.ends_with(&format!(".{tld}")))
}

/// Post-process results: junk/fragment/empty-snippet filtering, base-URL
/// dedup, snippet/title cleaning, and 1-based position renumbering.
#[allow(clippy::incompatible_msrv)]
fn post_process_results(items: &mut Vec<EngineSearchResult>) {
    // Filter junk, fragments, and short snippets, then dedup by base URL.
    let mut seen_bases: HashSet<String> = HashSet::new();
    items.retain(|r| {
        if r.url.contains("#:~:text=") {
            return false;
        }
        if crate::engine::router::is_translate_wrapper_url(&r.url) {
            return false;
        }
        if crate::harvest::is_junk_url(&r.url) {
            return false;
        }
        if r.snippet.trim().is_empty() {
            return false;
        }
        let base = gthings_common::dedup_key(&r.url);
        seen_bases.insert(base)
    });

    for (i, item) in items.iter_mut().enumerate() {
        // Clean snippet: strip trailing "Read more" and "...Read more".
        let snip = &item.snippet;
        if snip.ends_with("...Read more") {
            item.snippet = safe_truncate_end(snip, "...Read more");
        } else if snip.ends_with("Read more") {
            item.snippet = safe_truncate_end(snip, "Read more");
        }

        // Clean title: strip inline URLs.
        for prefix in &["https://", "http://"] {
            if let Some(pos) = item.title.find(prefix) {
                let safe_pos = item.title.floor_char_boundary(pos);
                item.title = item.title[..safe_pos].trim().to_string();
            }
        }
        // Detect appended domain at title end (e.g. "TitleExample.com"):
        // only strip when the trailing uppercase-starting token is a known
        // domain/TLD (ends with .com/.org/.net/...). Never strip arbitrary
        // uppercase words like "JavaScript", "iPhone", "eBay", or "Rust".
        let mut truncate_at: Option<usize> = None;
        for (i, c) in item.title.char_indices() {
            if i == 0 || !c.is_uppercase() {
                continue;
            }
            let prev = item.title[..i].chars().last().unwrap();
            if !(prev.is_lowercase() || prev == ')') {
                continue;
            }
            let suffix = &item.title[i..];
            if is_domain_suffix(suffix) {
                truncate_at = Some(i);
            }
        }
        if let Some(pos) = truncate_at {
            item.title = item.title[..pos].trim().to_string();
        }
        item.title = item.title.trim().to_string();

        // Re-number position sequentially (1-based).
        item.position = i + 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::SearchEngineError;

    fn result(title: &str, url: &str, snippet: &str, position: usize) -> EngineSearchResult {
        EngineSearchResult {
            title: title.to_string(),
            url: url.to_string(),
            snippet: snippet.to_string(),
            position,
            engine: SearchEngine::Google,
        }
    }

    #[test]
    fn extraction_js_substitutes_and_caches() {
        let a = extraction_js(5);
        let b = extraction_js(5);
        let c = extraction_js(7);
        assert!(a.contains("const count = 5;"));
        assert_eq!(a, b, "same count must reuse the cached template");
        assert!(c.contains("const count = 7;"));
        assert!(!c.contains("const count = 5;"));
    }

    #[test]
    fn should_scroll_skips_when_enough_results() {
        // count=10 → threshold count*2 = 20 (matches search_extract.js break).
        assert!(!should_scroll(20, 10), "exactly enough → skip scroll");
        assert!(!should_scroll(25, 10), "more than enough → skip scroll");
        assert!(should_scroll(19, 10), "below threshold → scroll");
        assert!(should_scroll(0, 10), "empty → scroll");
        // count=2 → threshold 4.
        assert!(should_scroll(3, 2), "3 < 4 → scroll");
        assert!(!should_scroll(4, 2), "4 >= 4 → skip scroll");
    }

    #[test]
    fn should_retry_gates_trailing_space_retry() {
        assert!(should_retry(true, "rust"), "empty bare query → retry");
        assert!(!should_retry(false, "rust"), "non-empty → no retry");
        assert!(!should_retry(true, "rust "), "already spaced → no retry");
        assert!(!should_retry(false, "rust "), "non-empty spaced → no retry");
    }

    #[test]
    fn captcha_url_detection() {
        assert!(is_captcha_url(
            "https://www.google.com/sorry/index?continue=https://www.google.com/search?q=x"
        ));
        assert!(is_captcha_url("https://google.com/sorry/?continue=https://www.google.com/"));
        assert!(!is_captcha_url("https://www.google.com/search?q=rust"));
        assert!(!is_captcha_url("https://example.com/page"));
        assert!(!is_captcha_url(""));
    }

    #[test]
    fn captcha_title_detection() {
        assert!(is_captcha_title("Accessibility help"));
        assert!(is_captcha_title("Google - Learn more about this page"));
        assert!(!is_captcha_title("rust - Google Search"));
        assert!(!is_captcha_title(""));
    }

    #[test]
    fn error_mapping() {
        assert!(matches!(
            map_cdp_error(CdpError::CaptchaBlocked {
                detail: "blocked".into()
            }),
            SearchEngineError::Captcha {
                engine: SearchEngine::Google,
                ..
            }
        ));
        let json_err = serde_json::from_str::<serde_json::Value>("{").unwrap_err();
        assert!(matches!(
            map_cdp_error(CdpError::Json(json_err)),
            SearchEngineError::Parse {
                engine: SearchEngine::Google,
                ..
            }
        ));
        assert!(matches!(
            map_cdp_error(CdpError::NavigationTimeout {
                url: "https://www.google.com".into(),
                timeout: 30
            }),
            SearchEngineError::Unavailable {
                engine: SearchEngine::Google,
                ..
            }
        ));
        assert!(matches!(
            map_cdp_error(CdpError::CdpCallFailed {
                method: "Page.navigate".into(),
                detail: "boom".into()
            }),
            SearchEngineError::Unavailable {
                engine: SearchEngine::Google,
                ..
            }
        ));
    }

    #[test]
    fn post_process_filters_junk_and_dedups() {
        let mut items = vec![
            result("Junk", "https://accounts.google.com/signin", "snippet", 1),
            result("Fragment", "https://example.com/page#:~:text=hello", "snippet", 2),
            result("Empty", "https://example.com/empty", "   ", 3),
            result("First", "https://example.com/page#1", "first snippet", 4),
            result("Duplicate", "https://example.com/page#2", "dup snippet", 5),
            result("Kept", "https://en.wikipedia.org/wiki/Entropy", "real snippet", 6),
        ];
        post_process_results(&mut items);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].url, "https://example.com/page#1");
        assert_eq!(items[0].snippet, "first snippet");
        assert_eq!(items[0].position, 1);
        assert_eq!(items[1].url, "https://en.wikipedia.org/wiki/Entropy");
        assert_eq!(items[1].position, 2);
    }

    #[test]
    fn post_process_drops_translate_wrappers() {
        let mut items = vec![
            result(
                "Translated",
                "https://example-com.translate.goog/page",
                "wrapper snippet",
                1,
            ),
            result(
                "Proxied",
                "https://translate.google.com/translate?u=https://example.org",
                "proxy snippet",
                2,
            ),
            result(
                "Redirected",
                "https://www.google.com/url?q=https://example.net/doc",
                "redirect snippet",
                3,
            ),
            result("Kept", "https://example.com/real", "real snippet", 4),
        ];
        post_process_results(&mut items);
        assert_eq!(items.len(), 1, "all translate/redirect wrappers filtered");
        assert_eq!(items[0].url, "https://example.com/real");
        assert_eq!(items[0].position, 1, "positions renumbered after filtering");
    }

    #[test]
    fn post_process_title_cleaning() {
        let mut items = vec![
            result("Title https://example.com/inline", "https://a.com", "s", 1),
            result("TitleExample.com", "https://b.com", "s", 2),
            result("Already clean", "https://c.com", "s", 3),
        ];
        post_process_results(&mut items);
        assert_eq!(items[0].title, "Title");
        assert_eq!(items[1].title, "Title");
        assert_eq!(items[2].title, "Already clean");
    }

    #[test]
    fn post_process_does_not_truncate_legitimate_titles() {
        let mut items = vec![
            result("JavaScript", "https://a.com", "s", 1),
            result("iPhone", "https://b.com", "s", 2),
            result("eBay", "https://c.com", "s", 3),
            result("Getting started with Rust", "https://d.com", "s", 4),
        ];
        post_process_results(&mut items);
        assert_eq!(items[0].title, "JavaScript");
        assert_eq!(items[1].title, "iPhone");
        assert_eq!(items[2].title, "eBay");
        assert_eq!(items[3].title, "Getting started with Rust");
    }

    #[test]
    fn is_domain_suffix_matches_only_domain_tokens() {
        assert!(is_domain_suffix("Example.com"));
        assert!(is_domain_suffix("Example.org"));
        assert!(is_domain_suffix("Example.io"));
        assert!(!is_domain_suffix("JavaScript"));
        assert!(!is_domain_suffix("Phone"));
        assert!(!is_domain_suffix("Bay"));
        assert!(!is_domain_suffix("Rust"));
        assert!(!is_domain_suffix("Wikipedia"));
    }

    #[test]
    fn post_process_snippet_cleaning() {
        let mut items = vec![
            result("A", "https://a.com", "some text...Read more", 1),
            result("B", "https://b.com", "other text Read more", 2),
            result("C", "https://c.com", "plain snippet", 3),
        ];
        post_process_results(&mut items);
        assert_eq!(items[0].snippet, "some text");
        assert_eq!(items[1].snippet, "other text");
        assert_eq!(items[2].snippet, "plain snippet");
    }
}
