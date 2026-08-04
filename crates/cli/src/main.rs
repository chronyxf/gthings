use clap::Parser;
use gthings_search::{EngineChoice, SearchEngine};
use std::time::Duration;

mod commands;

/// Top-level CLI struct with universal flags and subcommand.
#[derive(Parser)]
#[command(
    name = "gthings",
    version,
    about = "Browser automation and web research toolkit",
    disable_help_subcommand = true
)]
struct Cli {
    #[command(flatten)]
    universal: commands::UniversalFlags,

    #[command(subcommand)]
    command: Command,
}

#[derive(Parser)]
enum Command {
    /// Search Google and return results with strategy-based processing
    Search {
        #[command(flatten)]
        universal: commands::UniversalFlags,
        /// Search query or queries (multiple for parallel/harvest)
        queries: Vec<String>,
        /// Number of results per query
        #[arg(long, default_value = "5")]
        count: usize,
        /// Strategy: simple (single query, snippet results), parallel (multi-query, snippet results), harvest (full research pipeline: search + follow + extract content)
        #[arg(long, value_enum, default_value = "simple")]
        strategy: SearchStrategy,
        /// Engine: auto (default), brave (HTTP, no browser), bing (HTTP, no browser), google (needs CDP browser)
        #[arg(long, value_enum, default_value = "auto")]
        engine: EngineFlag,
        /// Extract content from result URLs (applies to parallel; harvest always follows)
        #[arg(long)]
        extract_results: bool,
        /// Max chars per extracted page (default: 40000)
        #[arg(long, default_value = "40000")]
        max_chars: usize,
        /// Dedup strategy for harvest (accepted values: url)
        #[arg(long, default_value = "url")]
        dedup: String,
        /// Rank strategy for harvest (accepted values: serp, authority, snippet, composite)
        #[arg(long, default_value = "composite")]
        rank: String,
        /// Number of top results to follow in harvest
        #[arg(long, default_value = "8")]
        follow_top: usize,
        /// Warn when tabs exceed this threshold (harvest)
        #[arg(long, default_value = "20")]
        warn_tabs: usize,
    },
    /// Check browser connection (JSON with status/running/stopped)
    Status {
        #[command(flatten)]
        universal: commands::UniversalFlags,
    },
    /// Update gthings to latest version
    Update,
    /// Extract content from any URL (auto-detects PDF, GitHub, arXiv, web)
    Extract {
        #[command(flatten)]
        universal: commands::UniversalFlags,
        url: String,
        #[arg(long, default_value = "40000")]
        max_chars: usize,
        #[arg(long, default_value = "0")]
        offset: usize,
    },

    /// Fetch compressed accessibility tree for a URL (AX tree)
    Ax {
        #[command(flatten)]
        universal: commands::UniversalFlags,
        url: String,
        /// Maximum number of nodes in compressed output (0 = unlimited)
        #[arg(long, default_value = "500")]
        max_nodes: usize,
    },
    /// Extract text from PDF at URL
    PdfUrl {
        #[command(flatten)]
        universal: commands::UniversalFlags,
        url: String,
        #[arg(long, default_value = "40000")]
        max_chars: usize,
        #[arg(long, default_value = "0")]
        offset: usize,
    },
    /// Extract text from local PDF file
    PdfFile {
        #[command(flatten)]
        universal: commands::UniversalFlags,
        path: std::path::PathBuf,
        #[arg(long, default_value = "40000")]
        max_chars: usize,
        #[arg(long, default_value = "0")]
        offset: usize,
    },
    /// Emit a machine-parseable JSON usage guide for AI agents (subcommands, strategies, engines, operators, output schema)
    Describe {
        #[command(flatten)]
        universal: commands::UniversalFlags,
    },
}

#[derive(clap::ValueEnum, Clone, Debug)]
enum SearchStrategy {
    /// simple: single query, snippet results
    Simple,
    /// parallel: multi-query, snippet results
    Parallel,
    /// harvest: full research pipeline (search + follow + extract content)
    Harvest,
}

#[derive(clap::ValueEnum, Clone, Debug)]
enum EngineFlag {
    /// auto (default): picks best engine; falls back to HTTP engines when no browser is available
    #[value(name = "auto")]
    Auto,
    /// brave: HTTP, no browser needed
    #[value(name = "brave")]
    Brave,
    /// bing: HTTP, no browser needed
    #[value(name = "bing")]
    Bing,
    /// google: needs CDP browser
    #[value(name = "google")]
    Google,
}

impl EngineFlag {
    fn to_choice(&self) -> EngineChoice {
        match self {
            EngineFlag::Auto => EngineChoice::Auto,
            EngineFlag::Brave => EngineChoice::Pin(SearchEngine::Brave),
            EngineFlag::Bing => EngineChoice::Pin(SearchEngine::Bing),
            EngineFlag::Google => EngineChoice::Pin(SearchEngine::Google),
        }
    }

    fn to_search_engine(&self) -> SearchEngine {
        match self {
            EngineFlag::Brave => SearchEngine::Brave,
            EngineFlag::Bing => SearchEngine::Bing,
            EngineFlag::Google => SearchEngine::Google,
            EngineFlag::Auto => panic!("EngineFlag::Auto has no concrete SearchEngine"),
        }
    }
}

/// Overall timeout for single-query searches (simple strategy, auto/pinned engine).
///
/// Google single queries can legitimately reach ~22s, and brave pin-mode can wait
/// up to 30s of pacing before the search even starts, so a 30s cap produced
/// spurious "timed out" failures on healthy runs. The error message derives from
/// this constant via `as_secs()` so the two cannot drift (same pattern as
/// `gthings_cdp::CONNECTION_TIMEOUT`).
const SEARCH_TIMEOUT: Duration = Duration::from_secs(60);

/// Run a future with a timeout, printing an error on timeout.
/// Returns `Ok(result)` on success, `Err(2)` on timeout.
async fn run_with_timeout<T, F: std::future::Future<Output = T>>(
    name: &str,
    secs: u64,
    fut: F,
) -> Result<T, i32> {
    if let Ok(result) = tokio::time::timeout(Duration::from_secs(secs), fut).await {
        Ok(result)
    } else {
        eprintln!("gthings: {name} timed out after {secs}s");
        Err(2)
    }
}

async fn handle_search_simple(
    universal: &mut commands::UniversalFlags,
    queries: Vec<String>,
    count: usize,
    engine: EngineFlag,
) -> i32 {
    if queries.is_empty() {
        commands::emit_output(
            None,
            Some((
                "EMPTY_QUERY",
                "Search query cannot be empty",
                "Provide a search term",
            )),
            universal.resolved_output(),
            universal.query.as_deref(),
        );
        return 1;
    }
    run_with_timeout(
        "search",
        SEARCH_TIMEOUT.as_secs(),
        commands::cmd_search(universal, &queries[0], count, engine),
    )
    .await
    .unwrap_or_else(|e| e)
}

async fn handle_search_parallel(
    universal: &mut commands::UniversalFlags,
    queries: Vec<String>,
    count: usize,
    extract_results: bool,
    max_chars: usize,
    engine: EngineFlag,
) -> i32 {
    run_with_timeout(
        "parallel search",
        60,
        commands::cmd_batch(universal, queries, count, extract_results, max_chars, engine),
    )
    .await
    .unwrap_or_else(|e| e)
}

#[allow(clippy::too_many_arguments)]
async fn handle_search_harvest(
    universal: &mut commands::UniversalFlags,
    queries: Vec<String>,
    dedup: String,
    rank: String,
    follow_top: usize,
    max_chars: usize,
    warn_tabs: usize,
    engine: EngineFlag,
) -> i32 {
    run_with_timeout(
        "harvest",
        120,
        commands::cmd_harvest(
            universal, queries, dedup, rank, follow_top, max_chars, warn_tabs, engine,
        ),
    )
    .await
    .unwrap_or_else(|e| e)
}

async fn handle_status(
    universal: &mut commands::UniversalFlags,
    global: &commands::UniversalFlags,
) -> i32 {
    universal.merge_from(global);
    run_with_timeout("status", 10, commands::cmd_status(universal))
        .await
        .unwrap_or_else(|e| e)
}

async fn handle_update() -> i32 {
    run_with_timeout("update", 60, commands::cmd_update())
        .await
        .unwrap_or_else(|e| e)
}

async fn handle_extract(
    universal: &mut commands::UniversalFlags,
    global: &commands::UniversalFlags,
    url: String,
    max_chars: usize,
    offset: usize,
) -> i32 {
    universal.merge_from(global);
    run_with_timeout(
        "extract",
        30,
        commands::cmd_extract(universal, &url, max_chars, offset),
    )
    .await
    .unwrap_or_else(|e| e)
}

async fn handle_ax(
    universal: &mut commands::UniversalFlags,
    global: &commands::UniversalFlags,
    url: String,
    max_nodes: usize,
) -> i32 {
    universal.merge_from(global);
    let max_nodes = if max_nodes == 0 {
        None
    } else {
        Some(max_nodes)
    };
    run_with_timeout("ax", 30, commands::cmd_ax(universal, &url, max_nodes))
        .await
        .unwrap_or_else(|e| e)
}

async fn handle_pdf_url(
    universal: &mut commands::UniversalFlags,
    global: &commands::UniversalFlags,
    url: String,
    max_chars: usize,
    offset: usize,
) -> i32 {
    universal.merge_from(global);
    run_with_timeout(
        "pdf url",
        30,
        commands::cmd_pdf_url(universal, &url, max_chars, offset),
    )
    .await
    .unwrap_or_else(|e| e)
}

async fn handle_pdf_file(
    universal: &mut commands::UniversalFlags,
    global: &commands::UniversalFlags,
    path: std::path::PathBuf,
    max_chars: usize,
    offset: usize,
) -> i32 {
    universal.merge_from(global);
    run_with_timeout(
        "pdf file",
        15,
        commands::cmd_pdf_file(universal, &path, max_chars, offset),
    )
    .await
    .unwrap_or_else(|e| e)
}

/// Emit a machine-parseable structured usage guide as JSON so AI agents can
/// self-discover the full CLI capability at runtime. Respects `--output json`.
fn handle_describe(universal: &commands::UniversalFlags) -> i32 {
    let guide = build_describe_guide();
    let formatted =
        commands::format_output(&guide, universal.resolved_output(), universal.query.as_deref());
    println!("{formatted}");
    0
}

/// Build the structured usage guide consumed by `gthings describe`.
fn build_describe_guide() -> serde_json::Value {
    serde_json::json!({
        "tool": "gthings",
        "description": "Multi-engine web search tool for AI agents: search, extract, and harvest web content via CDP.",
        "subcommands": {
            "search": {
                "purpose": "Search one or more engines and return results with strategy-based processing.",
                "flags": {
                    "queries": "Positional search term(s); multiple for parallel/harvest.",
                    "--count": "Number of results per query (default 5).",
                    "--strategy": "simple | parallel | harvest (default simple).",
                    "--engine": "auto | brave | bing | google (default auto).",
                    "--extract-results": "Extract content from result URLs (parallel; harvest always follows).",
                    "--max-chars": "Max chars per extracted page (default 40000).",
                    "--dedup": "Dedup strategy for harvest (accepted: url).",
                    "--rank": "Rank strategy for harvest (accepted: serp, authority, snippet, composite).",
                    "--follow-top": "Number of top results to follow in harvest (default 8).",
                    "--warn-tabs": "Warn when tabs exceed this threshold in harvest (default 20)."
                }
            },
            "status": { "purpose": "Check browser connection (JSON with status/running/stopped)." },
            "update": { "purpose": "Update gthings to the latest version." },
            "extract": {
                "purpose": "Extract content from any URL (auto-detects PDF, GitHub, arXiv, web).",
                "flags": {
                    "url": "Positional URL to extract.",
                    "--max-chars": "Max chars to extract (default 40000).",
                    "--offset": "Content offset (default 0)."
                }
            },
            "ax": {
                "purpose": "Fetch compressed accessibility tree for a URL (AX tree).",
                "flags": {
                    "url": "Positional URL.",
                    "--max-nodes": "Max nodes in compressed output, 0 = unlimited (default 500)."
                }
            },
            "pdf-url": {
                "purpose": "Extract text from PDF at URL.",
                "flags": { "url": "Positional URL.", "--max-chars": "default 40000.", "--offset": "default 0." }
            },
            "pdf-file": {
                "purpose": "Extract text from local PDF file.",
                "flags": { "path": "Positional file path.", "--max-chars": "default 40000.", "--offset": "default 0." }
            },
            "describe": { "purpose": "Emit this machine-parseable JSON usage guide." }
        },
        "strategies": {
            "simple": { "when": "Single query, snippet results. Fastest; use for quick lookups." },
            "parallel": { "when": "Multiple queries in parallel, snippet results. Use to broaden coverage across queries." },
            "harvest": { "when": "Full research pipeline: search + follow + extract content. Use for deep research on a topic." }
        },
        "engines": {
            "auto": { "transport": "auto", "note": "Default. Picks best engine; falls back to HTTP engines (brave, bing) when no browser is available." },
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
            "gthings search 'rust async' --strategy harvest --rank composite --dedup url",
            "gthings search 'site:github.com rust' --engine google",
            "gthings extract https://example.com --max-chars 100000",
            "gthings describe --output json"
        ]
    })
}

#[tokio::main]
async fn main() {
    let mut cli = Cli::parse();
    init_tracing(cli.universal.tracing_level());

    // Log panics to stderr instead of silent failure
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        eprintln!("gthings: panic: {info}");
        default_hook(info);
    }));

    let code = match cli.command {
        Command::Status { ref mut universal } => handle_status(universal, &cli.universal).await,
        Command::Update => handle_update().await,
        Command::Search {
            ref mut universal,
            queries,
            count,
            strategy,
            engine,
            extract_results,
            max_chars,
            dedup,
            rank,
            follow_top,
            warn_tabs,
        } => {
            universal.merge_from(&cli.universal);
            match strategy {
                SearchStrategy::Simple => {
                    handle_search_simple(universal, queries, count, engine).await
                }
                SearchStrategy::Parallel => {
                    handle_search_parallel(
                        universal,
                        queries,
                        count,
                        extract_results,
                        max_chars,
                        engine,
                    )
                    .await
                }
                SearchStrategy::Harvest => {
                    handle_search_harvest(
                        universal, queries, dedup, rank, follow_top, max_chars, warn_tabs, engine,
                    )
                    .await
                }
            }
        }

        Command::Extract {
            ref mut universal,
            url,
            max_chars,
            offset,
        } => handle_extract(universal, &cli.universal, url, max_chars, offset).await,
        Command::Ax {
            ref mut universal,
            url,
            max_nodes,
        } => handle_ax(universal, &cli.universal, url, max_nodes).await,
        Command::PdfUrl {
            ref mut universal,
            url,
            max_chars,
            offset,
        } => handle_pdf_url(universal, &cli.universal, url, max_chars, offset).await,
        Command::PdfFile {
            ref mut universal,
            path,
            max_chars,
            offset,
        } => handle_pdf_file(universal, &cli.universal, path, max_chars, offset).await,
        Command::Describe { ref universal } => handle_describe(universal),
    };
    std::process::exit(code);
}

/// Initialize the tracing subscriber, using the given log level string.
fn init_tracing(level: &str) {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(level)),
        )
        .with_writer(std::io::stderr)
        .without_time()
        .init();
}
