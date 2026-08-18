use gthings_common::provenance::ExtractionMethod;

use crate::engine::router::dispatch::effective_query;
use crate::engine::router::mapping::{
    has_non_latin_script, is_dictionary_junk, is_translate_wrapper_url, map_engine_results,
};
use crate::engine::{EngineMode, EngineSearchResult, SearchEngine};

use super::engine_result;

#[test]
fn translate_wrapper_url_detection() {
    assert!(is_translate_wrapper_url(
        "https://example-com.translate.goog/_x_tr_sl=en&_x_tr_tl=zh-CN"
    ));
    assert!(is_translate_wrapper_url(
        "https://translate.google.com/translate?sl=auto&tl=zh-CN&u=example.com"
    ));
    assert!(is_translate_wrapper_url(
        "https://www.google.com/url?q=https://example.com/doc&sa=U"
    ));
    assert!(!is_translate_wrapper_url("https://example.com/doc"));
    assert!(!is_translate_wrapper_url(""));
    // Structure-based matching: a bare "/url?q=" substring elsewhere must
    // not false-positive.
    assert!(!is_translate_wrapper_url(
        "https://example.com/url?q=not-a-wrapper"
    ));
    assert!(!is_translate_wrapper_url(
        "https://translate.example.com/translate?u=example.org"
    ));
    assert!(!is_translate_wrapper_url(
        "https://www.google.com/search?q=url%3Fq%3Dtest"
    ));
}

#[test]
fn non_latin_script_detection() {
    // CJK Unified Ideographs, Hiragana, Katakana, and Hangul all count.
    assert!(has_non_latin_script("中国 - 百度百科"));
    assert!(has_non_latin_script("ひらがなのページ"));
    assert!(has_non_latin_script("カタカナのページ"));
    assert!(has_non_latin_script("한국어 위키백과"));
    // Vietnamese (Latin Extended Additional) and Cyrillic junk.
    assert!(has_non_latin_script("Hiểu về Index"));
    assert!(has_non_latin_script("Понимание индекса"));
    assert!(has_non_latin_script("Ελληνική σελίδα"));
    assert!(has_non_latin_script("صفحة عربية"));
    assert!(has_non_latin_script("หน้าไทย"));
    assert!(has_non_latin_script("हिन्दी पृष्ठ"));
    // ASCII / Latin-script titles pass.
    assert!(!has_non_latin_script("Rust Programming Language"));
    assert!(!has_non_latin_script("Cafe - naive uber"));
    assert!(!has_non_latin_script(""));
}

#[test]
fn map_engine_results_drops_translate_wrappers() {
    let results = vec![
        engine_result(
            "Translated",
            "https://example-com.translate.goog/page",
            "wrapper snippet",
        ),
        engine_result(
            "Proxied",
            "https://translate.google.com/translate?u=https://example.org",
            "proxy snippet",
        ),
        engine_result(
            "Redirected",
            "https://www.google.com/url?q=https://example.net/doc&sa=U",
            "redirect snippet",
        ),
        engine_result("Kept", "https://example.com/real", "real snippet"),
    ];
    let mapped = map_engine_results(
        results,
        "https://example.com/search?q=test",
        7,
        EngineMode::Hybrid,
    );
    assert_eq!(mapped.len(), 1, "all translate/redirect wrappers filtered");
    assert_eq!(mapped[0].title, "Kept");
}

#[test]
fn map_engine_results_drops_non_latin_titles() {
    let results = vec![
        engine_result(
            "中国 百度百科",
            "https://baike.baidu.com/item/x",
            "中文结果",
        ),
        engine_result(
            "한국어 위키백과",
            "https://ko.wikipedia.org/wiki/러스트",
            "한국어",
        ),
        engine_result(
            "Rust (programming language) - Wikipedia",
            "https://en.wikipedia.org/wiki/Rust",
            "English result",
        ),
    ];
    let mapped = map_engine_results(
        results,
        "https://example.com/search?q=test",
        7,
        EngineMode::Hybrid,
    );
    assert_eq!(mapped.len(), 1, "localized (non-Latin) titles dropped");
    assert_eq!(mapped[0].title, "Rust (programming language) - Wikipedia");
    assert_eq!(
        mapped[0].position, 1,
        "positions renumbered after filtering"
    );
}

#[test]
fn map_engine_results_drops_non_latin_snippets() {
    // An English title with a non-English (Chinese) snippet must be
    // rejected — the snippet-level filter closes the blind spot where
    // only titles were checked.
    let results = vec![
        engine_result(
            "Rust programming language",
            "https://example.com/rust",
            "Rust 是一种系统编程语言",
        ),
        engine_result(
            "Cyrillic snippet",
            "https://example.org/cyr",
            "Понимание индекса",
        ),
        engine_result(
            "Vietnamese snippet",
            "https://example.net/vn",
            "Hiểu về Index",
        ),
        engine_result(
            "Kept",
            "https://example.com/kept",
            "A fully English snippet about Rust.",
        ),
    ];
    let mapped = map_engine_results(
        results,
        "https://example.com/search?q=test",
        7,
        EngineMode::Hybrid,
    );
    assert_eq!(
        mapped.len(),
        1,
        "non-Latin snippets dropped even with English titles"
    );
    assert_eq!(mapped[0].title, "Kept");
}

#[test]
fn dictionary_junk_detection() {
    // Known dictionary/definition domains (exact host and subdomain).
    assert!(is_dictionary_junk(
        "https://dictionary.cambridge.org/dictionary/english/rust",
        "Rust",
        "a reddish-brown substance",
    ));
    assert!(is_dictionary_junk(
        "https://www.merriam-webster.com/dictionary/rust",
        "Rust",
        "the reddish brittle coating",
    ));
    assert!(is_dictionary_junk(
        "https://www.dictionary.com/browse/rust",
        "Rust",
        "the red or orange coating",
    ));
    assert!(is_dictionary_junk(
        "https://www.scribbr.com/definitions/rust/",
        "Rust",
        "definition of rust",
    ));
    assert!(is_dictionary_junk(
        "https://www.thefreedictionary.com/rust",
        "Rust",
        "any of various metallic coatings",
    ));
    assert!(is_dictionary_junk(
        "https://www.vocabulary.com/dictionary/rust",
        "Rust",
        "a red or brown oxide coating",
    ));
    assert!(is_dictionary_junk(
        "https://www.collinsdictionary.com/dictionary/english/rust",
        "Rust",
        "a reddish-brown oxide coating",
    ));
    assert!(is_dictionary_junk(
        "https://www.oxfordlearnersdictionaries.com/definition/english/rust",
        "Rust",
        "a reddish-brown substance",
    ));
    // Title is a single word + "definition".
    assert!(is_dictionary_junk(
        "https://example.com/rust",
        "Rust definition",
        "some snippet",
    ));
    // Snippet contains "definition of".
    assert!(is_dictionary_junk(
        "https://example.com/rust",
        "Rust",
        "the definition of rust is a reddish coating",
    ));
    // Legitimate content is NOT over-filtered.
    assert!(!is_dictionary_junk(
        "https://en.wikipedia.org/wiki/Rust",
        "Rust (programming language) - Wikipedia",
        "Rust is a multi-paradigm programming language.",
    ));
    assert!(!is_dictionary_junk(
        "https://example.com/rust",
        "Rust programming language",
        "A systems language focused on safety.",
    ));
    assert!(!is_dictionary_junk(
        "https://example.com/rust",
        "Rust programming language definition",
        "A multi-word title mentioning definition is not a dictionary page.",
    ));
    assert!(!is_dictionary_junk("", "", ""));
}

#[test]
fn map_engine_results_drops_dictionary_junk() {
    let results = vec![
        engine_result(
            "Rust",
            "https://dictionary.cambridge.org/dictionary/english/rust",
            "a reddish-brown substance",
        ),
        engine_result(
            "Rust definition",
            "https://example.com/rust-def",
            "single-word title + definition",
        ),
        engine_result(
            "Rust",
            "https://example.org/rust",
            "the definition of rust is a coating",
        ),
        engine_result(
            "Rust (programming language) - Wikipedia",
            "https://en.wikipedia.org/wiki/Rust",
            "Rust is a multi-paradigm programming language.",
        ),
    ];
    let mapped = map_engine_results(
        results,
        "https://example.com/search?q=test",
        7,
        EngineMode::Hybrid,
    );
    assert_eq!(mapped.len(), 1, "dictionary/definition pages filtered");
    assert_eq!(mapped[0].title, "Rust (programming language) - Wikipedia");
}

#[test]
fn map_engine_results_filters_dedups_and_renumbers() {
    let results = vec![
        engine_result("Junk", "https://accounts.google.com/signin", "junk result"),
        engine_result(
            "Frag",
            "https://example.com/doc#:~:text=hi",
            "fragment link",
        ),
        engine_result("Empty", "https://example.org/empty", "   "),
        engine_result("Dup A", "https://example.com/doc#section1", "same base"),
        engine_result(
            "Dup B",
            "https://example.com/doc#section2",
            "same base again",
        ),
        engine_result("  Kept  ", "https://en.wikipedia.org/kept", "kept snippet"),
    ];

    let mapped = map_engine_results(
        results,
        "https://example.com/search?q=test",
        42,
        EngineMode::Hybrid,
    );

    assert_eq!(
        mapped.len(),
        2,
        "junk, fragment, empty snippet, and dup filtered"
    );
    // First base-URL occurrence wins; positions are renumbered 1-based.
    assert_eq!(mapped[0].title, "Dup A");
    assert_eq!(mapped[0].url, "https://example.com/doc#section1");
    assert_eq!(mapped[0].position, 1);
    assert_eq!(mapped[1].title, "Kept", "title trimmed");
    assert_eq!(mapped[1].position, 2);

    // Provenance: source_url is the result URL, method is Search.
    assert_eq!(
        mapped[1].provenance.source_url,
        "https://en.wikipedia.org/kept"
    );
    assert_eq!(mapped[1].provenance.method, ExtractionMethod::Search);
    assert_eq!(
        mapped[1].provenance.agent,
        gthings_common::user_agent::gthings_agent()
    );
    assert_eq!(mapped[1].provenance.duration_ms, 42);

    // Domain authority rounded to two decimals, kept as f64 (no float
    // artifacts), and a 0.9-tier domain (wikipedia.org) stays 0.9.
    assert_eq!(mapped[1].domain_authority, 0.9);
    assert_eq!(mapped[1].domain_authority.to_string(), "0.9");
}

#[test]
fn map_engine_results_dedups_normalized_url_and_title() {
    let results = vec![
        engine_result("Same", "https://www.Example.com/Path/", "first"),
        engine_result("Same", "https://example.com/Path", "second"),
        engine_result("Same", "https://example.com/Path?utm_source=x", "third"),
        engine_result("  Duplicate   Title ", "https://a.example/one", "title a"),
        engine_result("duplicate title", "https://b.example/two", "title b"),
        engine_result("Unique", "https://c.example/three", "unique"),
    ];
    let mapped = map_engine_results(
        results,
        "https://example.com/search?q=test",
        7,
        EngineMode::Hybrid,
    );
    // www/non-www + trailing slash + tracking query collapse to one base;
    // the two duplicate-title results collapse to one.
    assert_eq!(
        mapped.len(),
        3,
        "www, trailing slash, tracking, and dup titles deduped"
    );
    assert_eq!(mapped[0].title, "Same");
    assert_eq!(
        mapped[1].title, "Duplicate Title",
        "title trimmed + collapsed"
    );
    assert_eq!(mapped[2].title, "Unique");
}

#[test]
fn map_engine_results_filters_empty_titles() {
    let results = vec![
        engine_result("   ", "https://example.com/blank", "non-empty snippet"),
        engine_result("", "https://example.org/empty", "also non-empty"),
        engine_result("Kept", "https://example.net/kept", "kept snippet"),
    ];
    let mapped = map_engine_results(
        results,
        "https://example.com/search?q=test",
        7,
        EngineMode::Hybrid,
    );
    assert_eq!(mapped.len(), 1, "empty titles filtered even with a snippet");
    assert_eq!(mapped[0].title, "Kept");
}

#[test]
fn map_engine_results_populates_source_type() {
    let results = vec![
        engine_result("Repo", "https://github.com/rust-lang/rust", "github"),
        engine_result("Paper", "https://arxiv.org/abs/2301.00001", "arxiv"),
        engine_result("Doc", "https://example.com/guide.pdf", "pdf"),
        engine_result("Web", "https://example.com/page", "web"),
    ];
    let mapped = map_engine_results(
        results,
        "https://example.com/search?q=test",
        7,
        EngineMode::Hybrid,
    );
    assert_eq!(mapped.len(), 4);
    assert_eq!(mapped[0].source_type, "github");
    assert_eq!(mapped[1].source_type, "paper");
    assert_eq!(mapped[2].source_type, "pdf");
    assert_eq!(mapped[3].source_type, "web");
}

#[test]
fn effective_query_rewrites_per_engine() {
    let q = "(docker OR podman) compose AROUND(3)";
    // Bing does not support parens or AROUND(n): both are stripped, the
    // rest kept in order.
    assert_eq!(
        effective_query(SearchEngine::Bing, q),
        "docker OR podman compose"
    );
    // Google supports parens and AROUND(n): untouched.
    assert_eq!(effective_query(SearchEngine::Google, q), q);
}

#[test]
fn effective_query_passthrough_without_operators() {
    let q = "redis streams";
    for engine in [
        SearchEngine::Brave,
        SearchEngine::Bing,
        SearchEngine::Google,
    ] {
        assert_eq!(
            effective_query(engine, q),
            q,
            "no-operator query must be unchanged"
        );
    }
}

#[test]
fn map_engine_results_stamps_engine_on_each_result() {
    // Each mapped SearchResult must carry the engine that produced the
    // underlying EngineSearchResult, so consumers can attribute results to
    // the engine that actually served them.
    let results = vec![
        EngineSearchResult {
            title: "Brave hit".to_string(),
            url: "https://a.example/brave".to_string(),
            snippet: "brave snippet".to_string(),
            position: 1,
            engine: SearchEngine::Brave,
            score: 0.0,
            published_date: None,
            favicon: None,
        },
        EngineSearchResult {
            title: "Bing hit".to_string(),
            url: "https://b.example/bing".to_string(),
            snippet: "bing snippet".to_string(),
            position: 1,
            engine: SearchEngine::Bing,
            score: 0.0,
            published_date: None,
            favicon: None,
        },
        EngineSearchResult {
            title: "Tavily hit".to_string(),
            url: "https://c.example/tavily".to_string(),
            snippet: "tavily snippet".to_string(),
            position: 1,
            engine: SearchEngine::Tavily,
            score: 0.0,
            published_date: None,
            favicon: None,
        },
    ];
    let mapped = map_engine_results(
        results,
        "https://example.com/search?q=test",
        7,
        EngineMode::Hybrid,
    );
    assert_eq!(mapped.len(), 3, "no junk or dup filters interfere here");
    assert_eq!(mapped[0].engine, SearchEngine::Brave);
    assert_eq!(mapped[1].engine, SearchEngine::Bing);
    assert_eq!(mapped[2].engine, SearchEngine::Tavily);
}
