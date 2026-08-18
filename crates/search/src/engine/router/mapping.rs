//! Result filtering, dedup, and classification.
//!
//! Converts normalized engine results into crate-level [`SearchResult`]s,
//! filtering junk / wrapper / localized / dictionary results, deduping by base
//! URL and normalized title, renumbering positions, and attaching provenance.

use std::collections::HashSet;

use gthings_common::provenance::{ExtractionMethod, Provenance};

use crate::SearchResult;
use crate::engine::html::collapse_whitespace;
use crate::engine::{EngineMode, EngineSearchResult};

/// Whether `url` is a Google translate/redirect wrapper that must never be
/// surfaced as an organic result: the `translate.google.com/translate`
/// proxy, its `*.translate.goog` host, or Google's `/url?q=` redirect
/// wrapper.
pub(crate) fn is_translate_wrapper_url(url: &str) -> bool {
    let host = gthings_common::extract_host(url)
        .unwrap_or_default()
        .to_lowercase();
    // The translate.google.com proxy and any *.translate.goog host.
    if host == "translate.google.com" || host.ends_with(".translate.goog") {
        return true;
    }
    // Google's /url?q= redirect wrapper: host is google.com and the path is
    // exactly "/url" with a `q` query parameter.
    if host == "google.com" || host == "www.google.com" {
        let path = url.split('?').next().unwrap_or(url);
        if path.ends_with("/url") && url.contains("?q=") {
            return true;
        }
    }
    false
}

/// Whether `text` (a title or snippet) contains any character in a non-Latin
/// script that surfaces as junk for English queries: CJK Unified Ideographs
/// (U+4E00–U+9FFF), Hiragana (U+3040–U+309F), Katakana (U+30A0–U+30FF), Hangul
/// (U+AC00–U+D7AF), plus Latin Extended Additional (U+1E00–U+1EFF), Latin-1
/// diacritics (U+00C0–U+00FF), Cyrillic (U+0400–U+04FF), Greek (U+0370–U+03FF),
/// Arabic (U+0600–U+06FF), Thai (U+0E00–U+0E7F), and Devanagari (U+0900–U+097F).
/// Localized (non-English) results for English queries surface as junk; their
/// titles and snippets carry these scripts. Applied to both the title and the
/// snippet so a non-English snippet alone (e.g. a Chinese description under an
/// English title) is still rejected.
pub(crate) fn has_non_latin_script(text: &str) -> bool {
    text.chars().any(|c| {
        matches!(c as u32,
            0x3040..=0x309F   // Hiragana
            | 0x30A0..=0x30FF // Katakana
            | 0x4E00..=0x9FFF // CJK Unified Ideographs
            | 0xAC00..=0xD7AF // Hangul
            | 0x1E00..=0x1EFF // Latin Extended Additional (Vietnamese etc.)
            | 0x00C0..=0x00FF // Latin-1 diacritics
            | 0x0400..=0x04FF // Cyrillic
            | 0x0370..=0x03FF // Greek
            | 0x0600..=0x06FF // Arabic
            | 0x0E00..=0x0E7F // Thai
            | 0x0900..=0x097F // Devanagari
        )
    })
}

/// Domains that are dictionary/definition sites whose results are junk for
/// general English queries: they answer "what does X mean" rather than
/// substantive content. Matched on the exact host or any subdomain.
const DICTIONARY_DOMAINS: [&str; 8] = [
    "cambridge.org",
    "merriam-webster.com",
    "dictionary.com",
    "scribbr.com",
    "thefreedictionary.com",
    "vocabulary.com",
    "collinsdictionary.com",
    "oxfordlearnersdictionaries.com",
];

/// Whether `url`/`title`/`snippet` indicate a dictionary-definition page that
/// should be filtered as junk for general English queries.
///
/// Rejects results from known dictionary/definition domains, plus results whose
/// title is a single word followed by "definition" (e.g. "Rust definition") or
/// whose snippet contains "definition of". Deliberately narrow so legitimate
/// content that merely mentions a definition is not over-filtered.
pub(crate) fn is_dictionary_junk(url: &str, title: &str, snippet: &str) -> bool {
    let host = gthings_common::extract_host(url)
        .unwrap_or_default()
        .to_lowercase();
    if host_matches(&host, &DICTIONARY_DOMAINS) {
        return true;
    }
    let title_lower = title.to_lowercase();
    let snippet_lower = snippet.to_lowercase();
    // Title is a single word followed by "definition" (e.g. "Rust definition").
    if let Some(word) = title_lower.strip_suffix(" definition") {
        let word = word.trim();
        if !word.is_empty() && !word.contains(char::is_whitespace) {
            return true;
        }
    }
    // Snippet explicitly defines a term.
    if snippet_lower.contains("definition of") {
        return true;
    }
    false
}

/// Whether `url` carries a `#:~:text=` highlight fragment, which must never be
/// surfaced as an organic result URL.
pub(crate) fn is_fragment_url(url: &str) -> bool {
    url.contains("#:~:text=")
}

/// Whether `snippet` is empty after trimming — such results are dropped as
/// junk by the mapping and scrape post-processing paths.
pub(crate) fn is_empty_snippet(snippet: &str) -> bool {
    snippet.trim().is_empty()
}

/// Dedup `items` by a normalized base key, keeping the first occurrence of
/// each key (stable order).
///
/// Shared by the router mapping path, the CDP scrape post-processing, and the
/// harvest ranking dedup so there is a single source of truth for
/// first-occurrence-wins dedup.
pub(crate) fn dedup_by_base_key<T>(
    items: impl IntoIterator<Item = T>,
    key: impl Fn(&T) -> String,
) -> Vec<T> {
    let mut seen = HashSet::new();
    items
        .into_iter()
        .filter(|item| seen.insert(key(item)))
        .collect()
}

/// Convert normalized engine results into crate-level [`SearchResult`]s.
///
/// Filters junk URLs, translate/redirect wrappers, non-Latin (localized)
/// titles, `#:~:text=` fragments, and empty snippets; dedups by base URL
/// (before the first `#`) and normalized title; re-numbers positions 1-based;
/// trims titles; attaches per-result provenance (`source_url` = the result
/// URL, `method` = [`ExtractionMethod::Search`]); and rounds domain authority
/// to two decimals. `query` is the query's originating context used for
/// tracing only — provenance carries the result URL, matching
/// `crate::search` semantics.
// Consumed by the search facade and harvest phase_search, which are wired to
// this crate-internal helper separately.
pub(crate) fn map_engine_results(
    results: Vec<EngineSearchResult>,
    query: &str,
    duration_ms: u64,
    mode: EngineMode,
) -> Vec<SearchResult> {
    let survivors = filter_results(results);
    let deduped = dedup_results(survivors);

    let mut mapped: Vec<SearchResult> = deduped
        .into_iter()
        .map(|r| build_search_result(r, duration_ms, mode))
        .collect();
    for (i, r) in mapped.iter_mut().enumerate() {
        r.position = i + 1;
        // Backend-supplied scores win; otherwise derive from position.
        if r.score <= 0.0 {
            r.score = 1.0 / (i + 1) as f64;
        }
    }

    tracing::debug!(
        "mapped {} engine results (context {query:?}, {duration_ms}ms)",
        mapped.len()
    );
    mapped
}

/// Phase 1: filter out junk / wrapper / non-Latin / empty-title /
/// empty-snippet results.
fn filter_results(results: Vec<EngineSearchResult>) -> Vec<EngineSearchResult> {
    results
        .into_iter()
        .filter(|r| {
            !is_fragment_url(&r.url)
                && !is_translate_wrapper_url(&r.url)
                && !has_non_latin_script(&r.title)
                && !has_non_latin_script(&r.snippet)
                && !is_dictionary_junk(&r.url, &r.title, &r.snippet)
                && !crate::harvest::is_junk_url(&r.url)
                && !r.title.trim().is_empty()
                && !is_empty_snippet(&r.snippet)
        })
        .collect()
}

/// Phase 2: dedup by normalized base URL and by normalized title. The
/// normalized keys are owned Strings, so they outlive the survivors.
fn dedup_results(survivors: Vec<EngineSearchResult>) -> Vec<EngineSearchResult> {
    let mut seen_bases: HashSet<String> = HashSet::new();
    let mut seen_titles: HashSet<String> = HashSet::new();
    survivors
        .into_iter()
        .filter(|r| {
            let base = normalize_base_url(&r.url);
            let title = normalize_title(&r.title);
            seen_bases.insert(base) && seen_titles.insert(title)
        })
        .collect()
}

/// Phase 3: build a crate-level [`SearchResult`] from a kept engine result.
fn build_search_result(r: EngineSearchResult, duration_ms: u64, mode: EngineMode) -> SearchResult {
    let host = gthings_common::extract_host(&r.url).unwrap_or_default();
    let authority = (gthings_extraction::domain_authority(&host) as f64 * 100.0).round() / 100.0;
    let source_type = classify_source_type(&r.url);
    SearchResult {
        title: collapse_whitespace(&r.title),
        url: r.url.clone(),
        snippet: collapse_whitespace(&r.snippet),
        position: 0,
        provenance: Provenance {
            source_url: r.url,
            method: ExtractionMethod::Search,
            agent: gthings_common::user_agent::gthings_agent(),
            accessed_at: chrono::Utc::now(),
            duration_ms,
        },
        domain_authority: authority,
        source_type,
        engine: r.engine,
        score: r.score,
        published_date: r.published_date,
        favicon: r.favicon,
        mode,
    }
}

/// Normalize a title for dedup and uniform cleaning: trim and collapse runs of
/// whitespace into single spaces, then lowercase. Applied consistently to
/// titles from every engine so Google's own heuristic cannot diverge.
fn normalize_title(title: &str) -> String {
    collapse_whitespace(title).to_lowercase()
}

/// Normalize a result URL into a canonical base key for dedup. Delegates to
/// the shared [`gthings_common::dedup_key`] so there is a single source of
/// truth for URL base normalization across the engine layer.
fn normalize_base_url(url: &str) -> String {
    gthings_common::dedup_key(url)
}

/// Classify a result URL into a coarse `source_type` for citation metadata:
/// `github` for GitHub, `paper` for arXiv, `pdf` for direct PDF links, `news`
/// for known news outlets, `image` for direct image links / image hosts, and
/// `web` for everything else.
fn classify_source_type(url: &str) -> String {
    let host = gthings_common::extract_host(url)
        .unwrap_or_default()
        .to_lowercase();
    let path = url.split('?').next().unwrap_or(url).to_lowercase();
    if host == "github.com" || host.ends_with(".github.com") {
        "github".to_string()
    } else if host == "arxiv.org" || host.ends_with(".arxiv.org") {
        "paper".to_string()
    } else if path.ends_with(".pdf") {
        "pdf".to_string()
    } else if host_matches(&host, &NEWS_DOMAINS) {
        "news".to_string()
    } else if path.ends_with(".jpg")
        || path.ends_with(".jpeg")
        || path.ends_with(".png")
        || path.ends_with(".gif")
        || path.ends_with(".webp")
        || host_matches(&host, &IMAGE_DOMAINS)
    {
        "image".to_string()
    } else {
        "web".to_string()
    }
}

/// Known news-outlet domains (exact host or any subdomain).
const NEWS_DOMAINS: [&str; 4] = ["nytimes.com", "cnn.com", "reuters.com", "bbc.com"];

/// Known image-hosting domains (exact host or any subdomain).
const IMAGE_DOMAINS: [&str; 2] = ["imgur.com", "flickr.com"];

/// Whether `host` equals `domain` or is a subdomain of it.
fn host_matches(host: &str, domains: &[&str]) -> bool {
    domains
        .iter()
        .any(|d| host == *d || host.ends_with(&format!(".{d}")))
}
