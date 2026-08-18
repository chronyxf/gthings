//! Per-engine search-operator support.
//!
//! [`rewrite`] parses the search operators found in a raw query (`site:`,
//! `-exclusion`, `"exact phrase"`, `OR`/`AND`, parentheses, `AROUND(n)`,
//! numeric ranges, wildcards, `filetype:`, `intitle:`, ...) and strips the
//! operators a target engine does not support, per the canonical capability
//! matrix. Queries without operators pass through byte-for-byte unchanged.

use std::borrow::Cow;

use super::SearchEngine;

/// Keyword operators recognized as `keyword:value` pairs, case-insensitive.
const KEYVALUE_KEYWORDS: &[&str] = &[
    "site",
    "filetype",
    "intitle",
    "allintitle",
    "inurl",
    "allinurl",
    "intext",
    "related",
    "cache",
    "define",
    "before",
    "after",
    "near",
    "inanchor",
    "inbody",
    "loc",
    "language",
];

/// Token kinds produced by [`classify`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenKind {
    /// Contains a double quote: exact phrase (may contain spaces).
    Quoted,
    /// Starts with `-` followed by non-space: `-term` or `-site:...`.
    Exclusion,
    /// `keyword:value` operator with a recognized keyword.
    KeyValue,
    /// `AROUND(n)` proximity operator.
    Around,
    /// Contains `..` (numeric range, e.g. `$200..1000`).
    Range,
    /// `OR` / `AND` (case-insensitive).
    Boolean,
    /// Contains `(` or `)` grouping.
    Paren,
    /// Contains `*` wildcard.
    Wildcard,
    /// Everything else.
    Plain,
}

/// Split `query` on whitespace, treating quote-enclosed segments as single
/// units so `"context deadline exceeded" site:github.com` yields two tokens.
fn tokenize(query: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut start = 0usize;
    let mut in_quote = false;
    for (i, b) in query.bytes().enumerate() {
        match b {
            b'"' => in_quote = !in_quote,
            b' ' | b'\t' | b'\n' | b'\r' if !in_quote => {
                if i > start {
                    tokens.push(&query[start..i]);
                }
                start = i + 1;
            }
            _ => {}
        }
    }
    if start < query.len() {
        tokens.push(&query[start..]);
    }
    tokens
}

/// The keyword part of a `keyword:value` token, if it has a `:`.
fn keyvalue_keyword(token: &str) -> Option<&str> {
    let idx = token.find(':')?;
    (idx > 0).then(|| &token[..idx])
}

/// Whether `token` is `AROUND(digits)`, case-insensitive.
fn is_around(token: &str) -> bool {
    let t = token.as_bytes();
    if t.len() < 8 || !t[..7].eq_ignore_ascii_case(b"AROUND(") || t[t.len() - 1] != b')' {
        return false;
    }
    let inner = &t[7..t.len() - 1];
    !inner.is_empty() && inner.iter().all(|b| b.is_ascii_digit())
}

/// Classify a single token.
fn classify(token: &str) -> TokenKind {
    if token.starts_with('-') && token.len() > 1 {
        return TokenKind::Exclusion;
    }
    if token.contains('"') {
        return TokenKind::Quoted;
    }
    if let Some(kw) = keyvalue_keyword(token) {
        if KEYVALUE_KEYWORDS.iter().any(|k| k.eq_ignore_ascii_case(kw)) {
            return TokenKind::KeyValue;
        }
    }
    if is_around(token) {
        return TokenKind::Around;
    }
    if token.contains("..") {
        return TokenKind::Range;
    }
    if token.eq_ignore_ascii_case("or") || token.eq_ignore_ascii_case("and") {
        return TokenKind::Boolean;
    }
    if token.contains('(') || token.contains(')') {
        return TokenKind::Paren;
    }
    if token.contains('*') {
        return TokenKind::Wildcard;
    }
    TokenKind::Plain
}

/// Whether `engine` supports the `keyword:` operator.
fn kw_supported(engine: SearchEngine, kw: &str) -> bool {
    match engine {
        // Google keeps everything except the deprecated cache: and define:
        // (Google removed the define: operator in 2024 — it returns no results).
        SearchEngine::Google => {
            !kw.eq_ignore_ascii_case("cache") && !kw.eq_ignore_ascii_case("define")
        }
        SearchEngine::Brave => [
            "site", "filetype", "intitle", "inurl", "intext", "before", "after",
        ]
        .iter()
        .any(|k| kw.eq_ignore_ascii_case(k)),
        // Bing uses an RSS backend (bing.com/search?format=rss) that does NOT
        // honor advanced operators. `near:` actively degrades results (it
        // mangles the query into a single-word search), and `site:`, `after:`,
        // `before:`, `filetype:`, `intitle:`, `inurl:`, `intext:`, `inanchor:`,
        // `inbody:`, `loc:`, `language:`, `allintitle:`, `allinurl:`,
        // `related:`, `define:`, `cache:` are all ignored. Only exclusions
        // (`-term`), quoted phrases, and plain tokens survive.
        SearchEngine::Bing => false,
        // Paid API backends: forward keyword operators to the provider and let
        // it decide — neither docs a strict unsupported set, so pass through.
        SearchEngine::BraveApi | SearchEngine::Tavily => true,
    }
}

/// Collapse runs of multiple spaces inside a string to single spaces.
fn collapse_spaces(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for c in s.chars() {
        if c == ' ' && prev_space {
            continue;
        }
        prev_space = c == ' ';
        out.push(c);
    }
    out
}

/// True when `query` contains any character that could begin a search
/// operator. Used as a cheap gate to skip tokenizing/rebuilding queries that
/// are guaranteed to pass through unchanged.
fn has_operator_chars(query: &str) -> bool {
    query
        .bytes()
        .any(|b| matches!(b, b'"' | b'-' | b':' | b'(' | b')' | b'*'))
        || query.contains("..")
}

/// Collect plain tokens (lowercased) in order, marking each as a duplicate
/// when the same plain keyword appeared earlier (case-insensitive). This is
/// the single tokenize/classify/dedup routine shared by the fast-path
/// duplicate check and the tokenized rewrite, so both agree on what counts
/// as a repeated plain keyword.
fn plain_tokens(query: &str) -> Vec<(String, bool)> {
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for token in tokenize(query) {
        if classify(token) != TokenKind::Plain {
            continue;
        }
        let lower = token.to_lowercase();
        let dup = seen.contains(&lower);
        if !dup {
            seen.push(lower.clone());
        }
        out.push((lower, dup));
    }
    out
}

/// True when `query` repeats a plain keyword case-insensitively. Only
/// consulted when no operator chars are present; used to route
/// duplicate-bearing queries through the tokenized dedup path while keeping
/// duplicate-free queries on the borrowing fast path.
fn has_duplicate_plain_tokens(query: &str) -> bool {
    plain_tokens(query).iter().any(|(_, dup)| *dup)
}

/// Rewrite `query` for `engine`, dropping operators the engine does not
/// support while keeping supported tokens in original order, joined by
/// single spaces. Operator keyword case is preserved as typed.
///
/// Repeated plain keywords are deduplicated case-insensitively (first
/// occurrence wins, original order preserved) so queries stay focused on
/// distinct concepts. Non-plain tokens (exclusions, quoted phrases,
/// `keyword:` operators, ranges, ...) are never deduplicated.
///
/// Returns [`Cow::Borrowed`] when the query passes through unchanged (no
/// operators, no duplicate keywords, or empty input) and [`Cow::Owned`]
/// only when operators are stripped or keywords deduplicated. If every
/// token is stripped, the original raw query is returned as a fallback.
pub fn rewrite(query: &str, engine: SearchEngine) -> Cow<'_, str> {
    // Empty/whitespace queries pass through untouched.
    if query.trim().is_empty() {
        return Cow::Borrowed(query);
    }

    // Fast path: no operator trigger chars and no repeated plain keywords →
    // passthrough unchanged without tokenizing or rebuilding. Queries with
    // duplicate keywords still need dedup, so they route through the
    // tokenized path below.
    if !has_operator_chars(query) && !has_duplicate_plain_tokens(query) {
        return Cow::Borrowed(query);
    }

    let mut out: Vec<Cow<'_, str>> = Vec::with_capacity(query.len() / 4 + 1);
    // Reuses the shared tokenize/classify/dedup routine: `plain_tokens`
    // yields each Plain keyword in order with a `dup` flag marking whether
    // the same (case-insensitive) keyword appeared earlier. We walk it in
    // lockstep with the token stream, skipping only the duplicate
    // occurrences so the first occurrence of each keyword is retained.
    let plain = plain_tokens(query);
    let mut plain_idx = 0;
    for token in tokenize(query) {
        let kind = classify(token);
        if kind == TokenKind::Plain {
            let (_, dup) = plain[plain_idx];
            plain_idx += 1;
            if dup {
                continue;
            }
        }
        let keep = match kind {
            TokenKind::Quoted
            | TokenKind::Exclusion
            | TokenKind::Boolean
            | TokenKind::Range
            | TokenKind::Plain => true,
            TokenKind::KeyValue => {
                let kw = keyvalue_keyword(token).unwrap_or("");
                kw_supported(engine, kw)
            }
            // AROUND: whole token dropped on engines without it.
            TokenKind::Around => engine == SearchEngine::Google,
            // Parens: kept on Google/Brave; '(' ')' chars stripped on Bing.
            TokenKind::Paren => true,
            // Wildcards: kept on Google; '*' chars stripped on Brave/Bing.
            TokenKind::Wildcard => true,
        };
        if !keep {
            continue;
        }

        // Borrow the token by default; only allocate when a token actually
        // changes (wildcard/paren strip), so operator-bearing queries that
        // pass through unchanged don't allocate per token.
        let mut t: Cow<'_, str> = Cow::Borrowed(token);
        if matches!(
            engine,
            SearchEngine::Brave
                | SearchEngine::Bing
                | SearchEngine::BraveApi
                | SearchEngine::Tavily
        ) && token.contains('*')
        {
            // Wildcard-in-phrase (and standalone `*`) is unsupported:
            // remove '*' chars; collapse spaces left inside quoted phrases.
            let stripped = t.replace('*', "");
            t = if t.contains('"') {
                Cow::Owned(collapse_spaces(&stripped))
            } else {
                Cow::Owned(stripped)
            };
        }
        if engine == SearchEngine::Bing && kind == TokenKind::Paren {
            t = Cow::Owned(t.replace(['(', ')'], ""));
        }
        if !t.is_empty() {
            out.push(t);
        }
    }
    // Every token was stripped (e.g. Bing dropping all keyword operators):
    // fall back to the original raw query rather than returning "".
    if out.is_empty() {
        return Cow::Borrowed(query);
    }
    Cow::Owned(out.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Engines with real (partial or full) operator support.
    const ALL: [SearchEngine; 3] = [
        SearchEngine::Brave,
        SearchEngine::Bing,
        SearchEngine::Google,
    ];

    #[test]
    fn google_preserves_arounds_related_and_dates() {
        let q = "AROUND(3) related:react.dev before:2024 after:2025 hooks";
        assert_eq!(rewrite(q, SearchEngine::Google), q);
    }

    #[test]
    fn google_strips_cache_and_define() {
        let q = "react cache:react.dev define:rust hooks";
        assert_eq!(rewrite(q, SearchEngine::Google), "react hooks");
    }

    #[test]
    fn bing_strips_advanced_operators_keeps_exclusions_and_quoted() {
        let q = "rust near:3 cargo inanchor:docs inbody:guide loc:us language:en";
        assert_eq!(rewrite(q, SearchEngine::Bing), "rust cargo");
    }

    #[test]
    fn bing_strips_site_after_before_filetype_intitle() {
        let q = "site:github.com after:2025 before:2024 filetype:pdf intitle:x inurl:y intext:z";
        // All tokens are stripped; the empty-output fallback returns the
        // original raw query rather than "".
        assert_eq!(rewrite(q, SearchEngine::Bing), q);
    }

    #[test]
    fn bing_keeps_exclusions_and_quoted_phrases() {
        let q = "-reddit \"machine learning\" -site:github.com foo";
        assert_eq!(
            rewrite(q, SearchEngine::Bing),
            "-reddit \"machine learning\" -site:github.com foo"
        );
    }

    #[test]
    fn google_strips_define_but_keeps_related() {
        let q = "define:rust related:react.dev";
        assert_eq!(rewrite(q, SearchEngine::Google), "related:react.dev");
    }

    #[test]
    fn operator_query_without_strip_is_owned_but_tokens_borrowed() {
        // A query with operator trigger chars but nothing stripped must still
        // produce an owned result (joined), yet not allocate per kept token.
        let q = "site:github.com \"exact phrase\" -excluded";
        let out = rewrite(q, SearchEngine::Google);
        assert!(matches!(out, Cow::Owned(_)));
        assert_eq!(out.as_ref(), q);
    }

    #[test]
    fn brave_strips_related_and_around() {
        let q = "related:react.dev AROUND(3) site:react.dev \"hooks\" (foo)";
        assert_eq!(
            rewrite(q, SearchEngine::Brave),
            "site:react.dev \"hooks\" (foo)"
        );
    }

    #[test]
    fn bing_strips_parens_around_and_dates() {
        let q = "(docker OR podman) compose after:2025 AROUND(2)";
        assert_eq!(rewrite(q, SearchEngine::Bing), "docker OR podman compose");
    }

    #[test]
    fn universal_operators_kept_on_google_and_brave() {
        let q =
            "site:github.com \"exact phrase\" -excluded filetype:pdf intitle:x inurl:y intext:z";
        for engine in [SearchEngine::Google, SearchEngine::Brave] {
            assert_eq!(rewrite(q, engine), q);
        }
    }

    #[test]
    fn case_insensitive_keywords() {
        let q = "SITE:github.com FileType:pdf";
        for engine in [SearchEngine::Google, SearchEngine::Brave] {
            assert_eq!(rewrite(q, engine), q);
        }
    }

    #[test]
    fn exclusion_operator_form() {
        let q = "-site:github.com foo -bar";
        for engine in ALL {
            assert_eq!(rewrite(q, engine), q);
        }
    }

    #[test]
    fn wildcard_stripped_for_non_google() {
        let q = "how to * kubernetes";
        assert_eq!(rewrite(q, SearchEngine::Brave), "how to kubernetes");
        assert_eq!(rewrite(q, SearchEngine::Bing), "how to kubernetes");
        assert_eq!(rewrite(q, SearchEngine::Google), q);
    }

    #[test]
    fn quoted_phrase_with_spaces_is_one_unit() {
        let q = "\"context deadline exceeded\" site:github.com";
        for engine in [SearchEngine::Google, SearchEngine::Brave] {
            assert_eq!(rewrite(q, engine), q);
        }
        // Bing keeps the quoted phrase but strips the site: operator.
        assert_eq!(
            rewrite(q, SearchEngine::Bing),
            "\"context deadline exceeded\""
        );
    }

    #[test]
    fn empty_and_whitespace_query_unchanged() {
        assert_eq!(rewrite("", SearchEngine::Google), "");
        assert_eq!(rewrite("   ", SearchEngine::Brave), "   ");
    }

    #[test]
    fn no_operators_passthrough_unchanged() {
        let q = "redis streams";
        for engine in ALL {
            assert_eq!(rewrite(q, engine), q);
        }
    }

    #[test]
    fn allintitle_stripped_on_brave_bing_kept_on_google() {
        let q = "allintitle:foo x allinurl:bar";
        assert_eq!(rewrite(q, SearchEngine::Brave), "x");
        assert_eq!(rewrite(q, SearchEngine::Bing), "x");
        assert_eq!(rewrite(q, SearchEngine::Google), q);
    }

    #[test]
    fn ranges_kept_everywhere() {
        let q = "$200..1000 rust 3.10..3.13";
        for engine in ALL {
            assert_eq!(rewrite(q, engine), q);
        }
    }

    #[test]
    fn no_operator_query_is_borrowed() {
        let q = "redis streams";
        for engine in ALL {
            assert!(matches!(rewrite(q, engine), Cow::Borrowed(_)));
        }
    }

    #[test]
    fn operator_query_is_owned() {
        let q = "react cache:react.dev hooks";
        assert!(matches!(rewrite(q, SearchEngine::Google), Cow::Owned(_)));
    }

    #[test]
    fn keyword_order_and_dedup_for_google_and_brave() {
        // Order stability: keyword order is preserved verbatim for plain
        // queries (fast path) and across mixed queries (tokenized path).
        for engine in [SearchEngine::Google, SearchEngine::Brave] {
            assert_eq!(
                rewrite("kubernetes networking basics", engine),
                "kubernetes networking basics"
            );
            assert_eq!(
                rewrite("kubernetes networking basics site:github.com", engine),
                "kubernetes networking basics site:github.com"
            );
        }
        // Dedup: repeated plain keywords collapse case-insensitively, first
        // occurrence wins, for both Google and Brave.
        assert_eq!(
            rewrite("rust rust programming", SearchEngine::Google),
            "rust programming"
        );
        assert_eq!(rewrite("Rust rust", SearchEngine::Google), "Rust");
        assert_eq!(
            rewrite("rust rust programming", SearchEngine::Brave),
            "rust programming"
        );
        // Duplicates elsewhere (operator keywords, exclusions, quoted
        // phrases) are never deduplicated.
        assert_eq!(
            rewrite(
                "site:github.com site:github.com -foo -foo",
                SearchEngine::Google
            ),
            "site:github.com site:github.com -foo -foo"
        );
    }
}
