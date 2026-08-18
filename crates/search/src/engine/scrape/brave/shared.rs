//! Shared CDP scrape flow for the Brave and Google backends.
//!
//! The two backends differ only in their SERP URL, result-selector CSS,
//! CAPTCHA predicates, extraction template, and engine label; the navigation
//! → CAPTCHA check → extraction → lazy-load scroll → post-processing pipeline
//! is identical. This module holds the pipeline once, parameterized by
//! [`CdpSearchSpec`]. CAPTCHA detection stays per-engine (`is_captcha_url`/
//! `is_captcha_title` are injected via the spec so Google's `/sorry/`
//! predicates and Brave's `verify` predicates never collide).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use gthings_cdp::{CdpError, Session, Tab};
use gthings_common::strip_suffix_and_trim;
use serde::Deserialize;

use crate::engine::{EngineSearchResult, SearchEngine, SearchEngineError};

/// Everything one CDP scrape backend needs from the shared pipeline:
/// the SERP URL (query baked in), engine identity, message labels, the
/// result CSS selector used by the lazy-load scroll loop, the
/// engine-specific CAPTCHA predicates, and the extraction template.
pub(crate) struct CdpSearchSpec<'a> {
    /// Fully-built SERP URL for this query.
    pub url: String,
    /// Engine stamped into errors and results.
    pub engine: SearchEngine,
    /// Capitalized engine name for log/error messages ("Brave", "Google").
    pub page_label: &'a str,
    /// What the CAPTCHA title check calls the block ("block page" for
    /// Brave, "access-denied page" for Google).
    pub block_desc: &'a str,
    /// CSS selector counting organic results in the scroll loop.
    pub result_selector: &'a str,
    /// Engine-specific CAPTCHA URL predicate.
    pub is_captcha_url: fn(&str) -> bool,
    /// Engine-specific CAPTCHA title predicate.
    pub is_captcha_title: fn(&str) -> bool,
    /// Extraction template with a `__COUNT__` placeholder (see
    /// [`extraction_js`]).
    pub template: &'static str,
}

/// `brave_extract.js` embedded once.
pub(crate) const BRAVE_TEMPLATE: &str = include_str!("../../../../templates/brave_extract.js");
/// `search_extract.js` embedded once.
pub(crate) const GOOGLE_TEMPLATE: &str = include_str!("../../../../templates/search_extract.js");

/// Map CDP errors onto [`SearchEngineError`]: CAPTCHA → `Captcha`, JSON →
/// `Parse`, everything else (navigation timeouts included) → `Unavailable`.
pub(crate) fn map_cdp_error(err: CdpError, engine: SearchEngine) -> SearchEngineError {
    match err {
        CdpError::CaptchaBlocked { detail } => SearchEngineError::Captcha { engine, detail },
        CdpError::Json(e) => SearchEngineError::Parse {
            engine,
            detail: e.to_string(),
        },
        other => SearchEngineError::Unavailable {
            engine,
            detail: other.to_string(),
        },
    }
}

/// True when `suffix` looks like a domain token appended to a title, i.e.
/// it ends with a known TLD (`.com`, `.org`, `.net`, ...). Only such
/// suffixes are stripped from titles; arbitrary uppercase words are kept.
pub(crate) fn is_domain_suffix(suffix: &str) -> bool {
    const TLDS: &[&str] = &[
        "com", "org", "net", "io", "edu", "gov", "co", "uk", "us", "ca", "de", "fr", "jp", "au",
        "info", "biz", "me", "tv", "xyz", "dev", "app", "ai", "ru", "cn", "in", "br", "it", "es",
        "nl", "se", "pl", "ch", "at", "be", "dk", "fi", "no", "pt", "gr", "cz", "hu", "ro", "ua",
        "tr", "il", "kr", "tw", "hk", "sg", "my", "th", "vn", "ph", "id", "nz", "za", "mx", "ar",
        "cl", "pe", "ve",
    ];
    let lower = suffix.to_ascii_lowercase();
    TLDS.iter().any(|tld| lower.ends_with(&format!(".{tld}")))
}

/// Post-process results: junk/fragment/empty-snippet filtering, base-URL
/// dedup, snippet/title cleaning, and 1-based position renumbering.
pub(crate) fn post_process_results(items: &mut Vec<EngineSearchResult>) {
    // Filter junk, fragments, and short snippets, then dedup by base URL.
    items.retain(|r| {
        !crate::engine::router::is_fragment_url(&r.url)
            && !crate::engine::router::is_translate_wrapper_url(&r.url)
            && !crate::harvest::is_junk_url(&r.url)
            && !crate::engine::router::is_empty_snippet(&r.snippet)
    });
    *items = crate::engine::router::dedup_by_base_key(std::mem::take(items), |r| {
        gthings_common::dedup_key(&r.url)
    });

    for (i, item) in items.iter_mut().enumerate() {
        item.snippet = clean_snippet(&item.snippet);
        item.title = clean_title(&item.title);
        // Re-number position sequentially (1-based).
        item.position = i + 1;
    }
}

/// Clean a snippet: strip trailing "Read more" and "...Read more".
fn clean_snippet(snippet: &str) -> String {
    if snippet.ends_with("...Read more") {
        strip_suffix_and_trim(snippet, "...Read more")
    } else if snippet.ends_with("Read more") {
        strip_suffix_and_trim(snippet, "Read more")
    } else {
        snippet.to_string()
    }
}

/// Clean a title: strip inline URLs and any appended domain token (e.g.
/// "TitleExample.com"), keeping legitimate uppercase words like "JavaScript",
/// "iPhone", "eBay", or "Rust".
fn clean_title(title: &str) -> String {
    let mut title = title.to_string();
    // Strip inline URLs.
    for prefix in &["https://", "http://"] {
        if let Some(pos) = title.find(prefix) {
            let mut safe_pos = pos;
            while safe_pos > 0 && !title.is_char_boundary(safe_pos) {
                safe_pos -= 1;
            }
            title = title[..safe_pos].trim().to_string();
        }
    }
    // Detect appended domain at title end (e.g. "TitleExample.com"):
    // only strip when the trailing uppercase-starting token is a known
    // domain/TLD (ends with .com/.org/.net/...). Never strip arbitrary
    // uppercase words like "JavaScript", "iPhone", "eBay", or "Rust".
    let mut truncate_at: Option<usize> = None;
    for (i, c) in title.char_indices() {
        if i == 0 || !c.is_uppercase() {
            continue;
        }
        let prev = title[..i].chars().last().unwrap();
        if !(prev.is_lowercase() || prev == ')') {
            continue;
        }
        let suffix = &title[i..];
        if is_domain_suffix(suffix) {
            truncate_at = Some(i);
        }
    }
    if let Some(pos) = truncate_at {
        title = title[..pos].trim().to_string();
    }
    title.trim().to_string()
}

/// Run the extraction JS in `tab` and parse the returned organic results.
/// Parse failures are attributed to `engine` (`engine.as_str()` labels the
/// log line, e.g. "brave:", "bing_cdp:", "google:").
pub(crate) async fn extract_results(
    tab: &Tab,
    session: &Session,
    js: &str,
    engine: SearchEngine,
) -> Result<Vec<EngineSearchResult>, SearchEngineError> {
    let result = tab
        .evaluate(session, js)
        .await
        .map_err(|e| map_cdp_error(e, engine))?;
    let raw = result["result"]["value"].as_str();
    let json_str = raw.unwrap_or("[]");
    let raw_items: Vec<RawResult> = serde_json::from_str(json_str).map_err(|e| {
        let mut end = json_str.len().min(200);
        while end > 0 && !json_str.is_char_boundary(end) {
            end -= 1;
        }
        let preview = &json_str[..end];
        tracing::warn!(
            "{}: failed to parse results JSON: {e} (preview: {preview:?})",
            engine.as_str()
        );
        SearchEngineError::Parse {
            engine,
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
            engine,
            score: 0.0,
            published_date: None,
            favicon: None,
        })
        .collect())
}

/// Lazy-load scroll step in pixels per iteration.
const SCROLL_STEP_PX: u32 = 800;
/// Delay between scroll iterations (ms), letting lazy-loaded results arrive.
const SCROLL_DELAY_MS: u32 = 150;
/// Consecutive stable polls before the scroll loop stops early.
const SCROLL_STABLE_POLLS: u32 = 3;
/// The scroll target is `count * SCROLL_TARGET_MULTIPLIER` results — matches
/// the extraction templates' break threshold so we never scroll when the page
/// already has enough results.
const SCROLL_TARGET_MULTIPLIER: usize = 2;

/// JS template driving the lazy-load scroll loop. Scrolls in a bounded loop,
/// polling the DOM for the organic-result count after each step and stopping
/// once it stops growing (`stable` consecutive stable polls) or reaches the
/// `target` count. Placeholders: `iters`, `target`, `step`, `delay`, `stable`,
/// `selector`.
const SCROLL_JS_TEMPLATE: &str = r#"
(async () => {
  const iters = {iters};
  const target = {target};
  const step = {step};
  const delay = {delay};
  const stable = {stable};
  const selector = {selector};
  let stablePolls = 0;
  let last = 0;
  for (let i = 0; i < iters; i++) {
    window.scrollBy(0, step);
    await new Promise(r => setTimeout(r, delay));
    const n = document.querySelectorAll(selector).length;
    if (n >= target) break;
    if (n === last) {
      stablePolls++;
      if (stablePolls >= stable) break;
    } else {
      stablePolls = 0;
    }
    last = n;
  }
  return true;
})()
"#;

/// True when the first extraction returned fewer than `count * 2` results,
/// meaning a scroll may load more. Matches the extraction templates' break
/// threshold so we never scroll when the page already has enough results.
pub(crate) fn should_scroll(result_count: usize, count: usize) -> bool {
    result_count < count * SCROLL_TARGET_MULTIPLIER
}

/// True when an empty result set warrants a single trailing-space retry.
/// Gated so we never retry when the query already ends with a space (the
/// trailing-space variant can't help) or when results were found.
pub(crate) fn should_retry(results_empty: bool, query: &str) -> bool {
    results_empty && !query.ends_with(' ')
}

/// Run a full search with the shared trailing-space retry: search once, and
/// when the result set is empty and the query does not already end with a
/// space, retry ONCE with a trailing space (skipping the scroll to avoid a
/// second full CDP scroll cost). `build_spec` constructs the per-engine spec
/// for a query (Brave bakes only the query; Google also needs the count).
pub(crate) async fn search_with_retry(
    session: &Session,
    query: &str,
    count: usize,
    build_spec: impl Fn(&str) -> CdpSearchSpec<'static>,
) -> Result<Vec<EngineSearchResult>, SearchEngineError> {
    let results = search_once(session, &build_spec(query), count, true).await?;
    if should_retry(results.is_empty(), query) {
        let spaced = format!("{query} ");
        search_once(session, &build_spec(&spaced), count, false).await
    } else {
        Ok(results)
    }
}

/// Raw result shape produced by the extraction templates (title/url/
/// snippet/position; the `engine` field is filled in by the backend).
#[derive(Debug, Deserialize)]
struct RawResult {
    title: String,
    url: String,
    snippet: String,
    position: usize,
}

/// Cache of `__COUNT__`-substituted extraction templates, keyed by
/// (count, template) so each engine's template substitutes independently.
static SUBSTITUTED_TEMPLATES: OnceLock<Mutex<HashMap<(usize, &'static str), String>>> =
    OnceLock::new();

/// Return the extraction JS with `__COUNT__` substituted for `count`. The
/// substitution is cached per (template, count).
pub(crate) fn extraction_js(template: &'static str, count: usize) -> String {
    let cached = SUBSTITUTED_TEMPLATES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cached = cached.lock().unwrap();
    if let Some(js) = cached.get(&(count, template)) {
        return js.clone();
    }
    let js = template.replace("__COUNT__", &count.to_string());
    cached.insert((count, template), js.clone());
    js
}

/// Single search attempt (no retry) inside a freshly created background
/// tab. The tab is always closed before returning. `scroll` controls
/// whether the conditional lazy-load scroll runs (the retry attempt
/// passes `false` to avoid a redundant second scroll).
pub(crate) async fn search_once(
    session: &Session,
    spec: &CdpSearchSpec<'_>,
    count: usize,
    scroll: bool,
) -> Result<Vec<EngineSearchResult>, SearchEngineError> {
    let tab = session
        .create_background_tab()
        .await
        .map_err(|e| map_cdp_error(e, spec.engine))?;
    let outcome = search_in_tab(&tab, session, spec, count, scroll).await;
    if let Err(e) = session.close_tab(tab).await {
        tracing::warn!(
            "{}: failed to close background tab: {e}",
            spec.engine.as_str()
        );
    }
    outcome
}

/// Page-info JS evaluated once after navigation: URL and title in a single
/// CDP evaluate to save a round-trip.
const PAGE_INFO_JS: &str = "JSON.stringify({url: window.location.href, title: document.title})";

/// Navigate `tab` to the SERP and extract results: CAPTCHA check (URL and
/// title in a single CDP evaluate to save a round-trip), extraction,
/// conditional lazy-load scroll, then post-processing.
async fn search_in_tab(
    tab: &Tab,
    session: &Session,
    spec: &CdpSearchSpec<'_>,
    count: usize,
    scroll: bool,
) -> Result<Vec<EngineSearchResult>, SearchEngineError> {
    tab.navigate(session, &spec.url)
        .await
        .map_err(|e| map_cdp_error(e, spec.engine))?;

    // Check for a CAPTCHA/verification block — URL and title in a single
    // CDP evaluate to save a round-trip.
    let page_info = tab
        .evaluate(session, PAGE_INFO_JS)
        .await
        .map_err(|e| map_cdp_error(e, spec.engine))?;
    let info_str = page_info
        .get("result")
        .and_then(|r| r.get("value"))
        .and_then(|v| v.as_str())
        .unwrap_or("{}");
    let info: serde_json::Value = serde_json::from_str(info_str).unwrap_or_default();
    let current_url = info.get("url").and_then(|v| v.as_str()).unwrap_or("");
    let title = info.get("title").and_then(|v| v.as_str()).unwrap_or("");
    if (spec.is_captcha_url)(current_url) {
        tracing::warn!(
            "{} CAPTCHA/verification page detected at: {current_url}",
            spec.page_label
        );
        return Err(SearchEngineError::Captcha {
            engine: spec.engine,
            detail: format!(
                "{} served CAPTCHA page instead of search results: {current_url}",
                spec.page_label
            ),
        });
    }
    if (spec.is_captcha_title)(title) {
        tracing::warn!("{} {} detected: {title}", spec.page_label, spec.block_desc);
        return Err(SearchEngineError::Captcha {
            engine: spec.engine,
            detail: format!(
                "{} returned {} '{title}' instead of search results",
                spec.page_label, spec.block_desc
            ),
        });
    }

    let js = extraction_js(spec.template, count);

    let mut items = extract_results(tab, session, &js, spec.engine).await?;

    // Scroll down to trigger lazy loading of more organic results — but
    // only when the first extraction didn't already return enough. When
    // enough results are present the scroll is redundant and skipped
    // entirely. A single CDP evaluate runs the whole scroll sequence in
    // JS (bounded loop with a short async delay), so there are no
    // Rust-side sleeps.
    if scroll && should_scroll(items.len(), count) {
        let scroll_iterations = count.max(3);
        let target_count = count * SCROLL_TARGET_MULTIPLIER;
        // Scroll in a bounded loop, but after each scroll poll the DOM
        // for the organic-result count and only stop once it stops
        // growing (3 consecutive stable polls) or reaches the target.
        // This waits out lazy-loading that arrives after a longer delay
        // instead of re-extracting immediately.
        let selector = spec.result_selector;
        let scroll_js = SCROLL_JS_TEMPLATE
            .replace("{iters}", &scroll_iterations.to_string())
            .replace("{target}", &target_count.to_string())
            .replace("{step}", &SCROLL_STEP_PX.to_string())
            .replace("{delay}", &SCROLL_DELAY_MS.to_string())
            .replace("{stable}", &SCROLL_STABLE_POLLS.to_string())
            .replace("{selector}", selector);
        tab.evaluate(session, &scroll_js)
            .await
            .map_err(|e| map_cdp_error(e, spec.engine))?;
        items = extract_results(tab, session, &js, spec.engine).await?;
    }

    post_process_results(&mut items);

    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(title: &str, url: &str, snippet: &str, position: usize) -> EngineSearchResult {
        EngineSearchResult {
            title: title.to_string(),
            url: url.to_string(),
            snippet: snippet.to_string(),
            position,
            engine: SearchEngine::Brave,
            score: 0.0,
            published_date: None,
            favicon: None,
        }
    }

    #[test]
    fn error_mapping_uses_engine_label() {
        for engine in [SearchEngine::Brave, SearchEngine::Google] {
            assert!(matches!(
                map_cdp_error(
                    CdpError::CaptchaBlocked {
                        detail: "blocked".into()
                    },
                    engine
                ),
                SearchEngineError::Captcha { engine: e, .. } if e == engine
            ));
            let json_err = serde_json::from_str::<serde_json::Value>("{").unwrap_err();
            assert!(matches!(
                map_cdp_error(CdpError::Json(json_err), engine),
                SearchEngineError::Parse { engine: e, .. } if e == engine
            ));
            assert!(matches!(
                map_cdp_error(
                    CdpError::NavigationTimeout {
                        url: "https://example.com".into(),
                        timeout: 30
                    },
                    engine
                ),
                SearchEngineError::Unavailable { engine: e, .. } if e == engine
            ));
            assert!(matches!(
                map_cdp_error(
                    CdpError::CdpCallFailed {
                        method: "Page.navigate".into(),
                        detail: "boom".into()
                    },
                    engine
                ),
                SearchEngineError::Unavailable { engine: e, .. } if e == engine
            ));
        }
    }

    #[test]
    fn post_process_filters_junk_and_dedups() {
        let mut items = vec![
            result("Junk", "https://accounts.google.com/signin", "snippet", 1),
            result(
                "Fragment",
                "https://example.com/page#:~:text=hello",
                "snippet",
                2,
            ),
            result("Empty", "https://example.com/empty", "   ", 3),
            result("First", "https://example.com/page#1", "first snippet", 4),
            result("Duplicate", "https://example.com/page#2", "dup snippet", 5),
            result(
                "Kept",
                "https://en.wikipedia.org/wiki/Entropy",
                "real snippet",
                6,
            ),
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

    #[test]
    fn should_scroll_skips_when_enough_results() {
        // count=10 → threshold count*2 = 20 (matches the templates' break).
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
    fn extraction_js_substitutes_and_caches() {
        let a = extraction_js("const count = __COUNT__;", 5);
        let b = extraction_js("const count = __COUNT__;", 5);
        let c = extraction_js("const count = __COUNT__;", 7);
        assert!(a.contains("const count = 5;"));
        assert_eq!(a, b, "same count must reuse the cached template");
        assert!(c.contains("const count = 7;"));
        assert!(!c.contains("const count = 5;"));
    }
}
