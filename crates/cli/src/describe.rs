//! `gthings describe` — emit a machine-parseable structured usage guide as
//! JSON so AI agents can self-discover the full CLI capability at runtime.

use crate::util::{UniversalFlags, emit_success};

/// Single source of truth for the numeric defaults surfaced in the guide.
///
/// These mirror the `#[arg(default_value = ...)]` values in `crate::args` so
/// the guide never drifts from the real CLI defaults. Update here and in
/// `crate::args` together.
struct GuideDefaults {
    count: usize,
    max_chars: usize,
    follow_top: usize,
    warn_tabs: usize,
    max_nodes: usize,
    offset: usize,
}

const DEFAULTS: GuideDefaults = GuideDefaults {
    count: 5,
    max_chars: 40000,
    follow_top: 8,
    warn_tabs: 20,
    max_nodes: 500,
    offset: 0,
};

/// Emit the usage guide, respecting `--output json`.
pub(crate) fn handle_describe(universal: &UniversalFlags) -> i32 {
    let guide = build_describe_guide();
    emit_success(universal, guide);
    0
}

/// Build the structured usage guide consumed by `gthings describe`.
pub(crate) fn build_describe_guide() -> serde_json::Value {
    serde_json::json!({
        "tool": "gthings",
        "description": "Multi-engine web search tool for AI agents: search, extract, and harvest web content via CDP.",
        "subcommands": {
            "search": {
                "purpose": "Search one or more engines and return results with strategy-based processing.",
                "flags": {
                    "queries": "Positional search term(s); multiple for parallel/harvest.",
                    "--count": format!("Number of results per query (default {}).", DEFAULTS.count),
                    "--strategy": "simple | parallel | harvest (default simple).",
                    "--engine": "auto | brave | bing | google (default auto).",
                    "--extract-results": "Extract content from result URLs (parallel; harvest always follows).",
                    "--max-chars": format!("Max chars per extracted page (default {}).", DEFAULTS.max_chars),
                    "--rank": "Rank strategy for harvest (accepted: serp, authority, snippet, composite).",
                    "--follow-top": format!("Number of top results to follow in harvest (default {}).", DEFAULTS.follow_top),
                    "--warn-tabs": format!("Warn when tabs exceed this threshold in harvest (default {}).", DEFAULTS.warn_tabs)
                }
            },
            "status": { "purpose": "Check browser connection (JSON with status/running/stopped)." },
            "health": { "purpose": "Liveness probe: exit 0 if a CDP browser is running, exit 1 otherwise (no connect)." },
            "update": { "purpose": "Update gthings to the latest version." },
            "serve": { "purpose": "Run the HTTP :9080 daemon: bounded job queue, query cache, warm CDP pool, and SSE job events. Blocks until SIGTERM/SIGINT drains it." },
            "config": { "purpose": "Print the resolved env+defaults configuration as a standard envelope; Go validates boot assumptions against it." },
            "extract": {
                "purpose": "Extract content from any URL (auto-detects PDF, GitHub, arXiv, web).",
                "flags": {
                    "url": "Positional URL to extract.",
                    "--max-chars": format!("Max chars to extract (default {}).", DEFAULTS.max_chars),
                    "--offset": format!("Content offset (default {}).", DEFAULTS.offset)
                }
            },
            "ax": {
                "purpose": "Fetch compressed accessibility tree for a URL (AX tree).",
                "flags": {
                    "url": "Positional URL.",
                    "--max-nodes": format!("Max nodes in compressed output, 0 = unlimited (default {}).", DEFAULTS.max_nodes)
                }
            },
            "pdf-url": {
                "purpose": "Extract text from PDF at URL.",
                "flags": { "url": "Positional URL.", "--max-chars": format!("default {}.", DEFAULTS.max_chars), "--offset": format!("default {}.", DEFAULTS.offset) }
            },
            "pdf-file": {
                "purpose": "Extract text from local PDF file.",
                "flags": { "path": "Positional file path.", "--max-chars": format!("default {}.", DEFAULTS.max_chars), "--offset": format!("default {}.", DEFAULTS.offset) }
            },
            "describe": { "purpose": "Emit this machine-parseable JSON usage guide." }
        },
        "strategies": {
            "simple": { "when": "Single query, snippet results. Fastest; use for quick lookups." },
            "parallel": { "when": "Multiple queries in parallel, snippet results. Use to broaden coverage across queries." },
            "harvest": { "when": "Full research pipeline: search + follow + extract content. Use for deep research on a topic." }
        },
        "engines": {
            "auto": { "transport": "auto", "note": "Picks best engine; falls back to HTTP engines (brave, bing) when no browser is available." },
            "brave": { "transport": "HTTP", "note": "No browser needed." },
            "bing": { "transport": "HTTP", "note": "No browser needed. RSS backend ignores most advanced operators." },
            "google": { "transport": "CDP", "note": "Requires a CDP browser connection." }
        },
        "operators": {
            "site:": { "engines": ["google", "brave"], "note": "Restrict results to a domain." },
            "-exclusion": { "engines": ["google", "brave", "bing"], "note": "Exclude a term or site (e.g. -reddit, -site:github.com)." },
            "\"quoted\"": { "engines": ["google", "brave", "bing"], "note": "Exact phrase match." },
            "filetype:": { "engines": ["google", "brave"], "note": "Restrict to a file type (e.g. filetype:pdf)." },
            "intitle:": { "engines": ["google", "brave"], "note": "Term must appear in the title." },
            "inurl:": { "engines": ["google", "brave"], "note": "Term must appear in the URL." },
            "AROUND(n)": { "engines": ["google"], "note": "Proximity operator; Google only." },
            "before:": { "engines": ["google", "brave"], "note": "Results before a date." },
            "after:": { "engines": ["google", "brave"], "note": "Results after a date." }
        },
        "output_schema": {
            "status": "ok | error",
            "data": "Command result payload (null on error).",
            "error": "null on success, else {code, detail, hint}."
        },
        "examples": [
            "gthings search 'rust async' --strategy simple --engine brave",
            "gthings search 'rust async' 'tokio' --strategy parallel --extract-results",
            "gthings search 'rust async' --strategy harvest --rank composite",
            "gthings search 'site:github.com rust' --engine google",
            "gthings extract https://example.com --max-chars 100000",
            "gthings describe --output json"
        ]
    })
}
