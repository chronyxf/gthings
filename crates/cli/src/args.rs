//! CLI argument definitions: the top-level [`Cli`] struct, the [`Command`]
//! enum (ten subcommands), and the search strategy + engine flag enums with
//! their engine → choice/mode conversion helpers.
//!
//! Universal flags (`--output`, `--query`, `--cdp-port`, ...) are declared
//! `global = true` on [`util::UniversalFlags`] and flattened only at the top
//! level, so clap accepts them on every subcommand without per-subcommand
//! re-declaration.

use clap::Parser;
use gthings_search::{EngineChoice, SearchEngine};

use crate::util;

/// Top-level CLI struct with universal flags and subcommand.
#[derive(Parser)]
#[command(
    name = "gthings",
    version,
    about = "Browser automation and web research toolkit",
    disable_help_subcommand = true
)]
pub(crate) struct Cli {
    #[command(flatten)]
    pub(crate) universal: util::UniversalFlags,

    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Parser)]
pub(crate) enum Command {
    /// Search Google and return results with strategy-based processing
    Search {
        /// Search query or queries (multiple for parallel/harvest)
        queries: Vec<String>,
        /// Number of results per query
        #[arg(long, default_value = "5")]
        count: usize,
        /// Strategy: simple (single query, snippet results), parallel (multi-query, snippet results), harvest (full research pipeline: search + follow + extract content)
        #[arg(long, value_enum, default_value = "simple")]
        strategy: SearchStrategy,
        /// Engine: auto picks the best engine; brave/bing (HTTP) and google (CDP) pin one engine
        #[arg(long, value_enum, default_value = "auto")]
        engine: EngineFlag,
        /// Extract content from result URLs (applies to parallel; harvest always follows)
        #[arg(long)]
        extract_results: bool,
        /// Max chars per extracted page (default: 40000)
        #[arg(long, default_value = "40000")]
        max_chars: usize,
        /// Rank strategy for harvest (accepted values: serp, authority, snippet, composite)
        #[arg(long, value_enum, default_value = "composite")]
        rank: RankFlag,
        /// Number of top results to follow in harvest
        #[arg(long, default_value = "8")]
        follow_top: usize,
        /// Warn when tabs exceed this threshold (harvest)
        #[arg(long, default_value = "20")]
        warn_tabs: usize,
    },
    /// Check browser connection (JSON with status/running/stopped)
    Status,
    /// Liveness probe: exit 0 if a CDP browser is running, exit 1 otherwise (no connect)
    Health,
    /// Update gthings to latest version
    Update,
    /// Run the HTTP :9080 daemon (bounded job queue, query cache, warm CDP pool, SSE events). Blocking until SIGTERM/SIGINT drains it
    Serve,
    /// Print the resolved env+defaults configuration as an envelope (Go validates boot assumptions)
    Config,
    /// Extract content from any URL (auto-detects PDF, GitHub, arXiv, web)
    Extract {
        url: String,
        #[arg(long, default_value = "40000")]
        max_chars: usize,
        #[arg(long, default_value = "0")]
        offset: usize,
    },

    /// Fetch compressed accessibility tree for a URL (AX tree)
    Ax {
        url: String,
        /// Maximum number of nodes in compressed output (0 = unlimited)
        #[arg(long, default_value = "500")]
        max_nodes: usize,
    },
    /// Extract text from PDF at URL
    PdfUrl {
        url: String,
        #[arg(long, default_value = "40000")]
        max_chars: usize,
        #[arg(long, default_value = "0")]
        offset: usize,
    },
    /// Extract text from local PDF file
    PdfFile {
        path: std::path::PathBuf,
        #[arg(long, default_value = "40000")]
        max_chars: usize,
        #[arg(long, default_value = "0")]
        offset: usize,
    },
    /// Emit a machine-parseable JSON usage guide for AI agents (subcommands, strategies, engines, operators, output schema)
    Describe,
}

#[derive(clap::ValueEnum, Clone, Debug)]
pub(crate) enum SearchStrategy {
    /// simple: single query, snippet results
    Simple,
    /// parallel: multi-query, snippet results
    Parallel,
    /// harvest: full research pipeline (search + follow + extract content)
    Harvest,
}

#[derive(clap::ValueEnum, Clone, Debug, PartialEq, Eq)]
pub(crate) enum EngineFlag {
    /// auto: picks best engine; falls back to HTTP engines when no browser is available
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

/// Rank strategy for the harvest pipeline (clap-validated).
#[derive(clap::ValueEnum, Clone, Debug, PartialEq, Eq)]
pub(crate) enum RankFlag {
    /// serp: preserve search-engine result order
    Serp,
    /// authority: rank by domain authority
    Authority,
    /// snippet: rank by snippet length
    Snippet,
    /// composite: blend multiple signals (default)
    Composite,
}

/// Bundled `search` arguments passed from dispatch to the strategy splitter.
pub(crate) struct SearchArgs {
    pub(crate) queries: Vec<String>,
    pub(crate) count: usize,
    pub(crate) strategy: SearchStrategy,
    pub(crate) engine: EngineFlag,
    pub(crate) extract_results: bool,
    pub(crate) max_chars: usize,
    pub(crate) rank: RankFlag,
    pub(crate) follow_top: usize,
    pub(crate) warn_tabs: usize,
}

impl EngineFlag {
    pub(crate) fn to_choice(&self) -> EngineChoice {
        match self {
            EngineFlag::Auto => EngineChoice::Auto,
            EngineFlag::Brave => EngineChoice::Pin(SearchEngine::Brave),
            EngineFlag::Bing => EngineChoice::Pin(SearchEngine::Bing),
            EngineFlag::Google => EngineChoice::Pin(SearchEngine::Google),
        }
    }

    /// Pinned engine for strategies that need a concrete engine (harvest).
    /// `None` means "route automatically" — only `auto`.
    pub(crate) fn to_search_engine(&self) -> Option<SearchEngine> {
        match self {
            EngineFlag::Brave => Some(SearchEngine::Brave),
            EngineFlag::Bing => Some(SearchEngine::Bing),
            EngineFlag::Google => Some(SearchEngine::Google),
            EngineFlag::Auto => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse a `search` invocation and return the resolved `--engine` flag.
    fn engine_for(args: &[&str]) -> EngineFlag {
        let cli = match Cli::try_parse_from(args) {
            Ok(c) => c,
            Err(e) => panic!("failed to parse {args:?}: {e}"),
        };
        match cli.command {
            Command::Search { engine, .. } => engine,
            _ => panic!("expected a Search command for {args:?}"),
        }
    }

    /// The default engine must be `auto`. Routing modes (`free`/`hybrid`/`api`)
    /// are a daemon concern (`GTHINGS_ENGINE_MODE`), no longer CLI flags.
    #[test]
    fn engine_defaults_to_auto() {
        assert_eq!(engine_for(&["gthings", "search", "rust"]), EngineFlag::Auto);
    }

    #[test]
    fn engine_keeps_pinned_and_auto_values() {
        assert_eq!(
            engine_for(&["gthings", "search", "rust", "--engine", "auto"]),
            EngineFlag::Auto
        );
        assert_eq!(
            engine_for(&["gthings", "search", "rust", "--engine", "brave"]),
            EngineFlag::Brave
        );
        assert_eq!(
            engine_for(&["gthings", "search", "rust", "--engine", "bing"]),
            EngineFlag::Bing
        );
        assert_eq!(
            engine_for(&["gthings", "search", "rust", "--engine", "google"]),
            EngineFlag::Google
        );
    }

    #[test]
    fn engine_rejects_unknown_values() {
        assert!(
            Cli::try_parse_from(["gthings", "search", "rust", "--engine", "yahoo"]).is_err(),
            "unknown engine value must be rejected"
        );
        assert!(
            Cli::try_parse_from(["gthings", "search", "rust", "--engine", "free"]).is_err(),
            "routing modes are no longer accepted on the CLI"
        );
    }

    #[test]
    fn engine_flags_map_to_choice() {
        assert_eq!(EngineFlag::Auto.to_choice(), EngineChoice::Auto);
        assert_eq!(
            EngineFlag::Brave.to_choice(),
            EngineChoice::Pin(SearchEngine::Brave)
        );
        assert_eq!(
            EngineFlag::Bing.to_choice(),
            EngineChoice::Pin(SearchEngine::Bing)
        );
        assert_eq!(
            EngineFlag::Google.to_choice(),
            EngineChoice::Pin(SearchEngine::Google)
        );
    }

    /// Pinned engines still pin a concrete backend; `auto` routes
    /// automatically (`None`) so the router's env-resolved mode picks the
    /// priority.
    #[test]
    fn pinned_engines_pin_while_auto_routes() {
        assert_eq!(EngineFlag::Auto.to_search_engine(), None);
        assert_eq!(
            EngineFlag::Brave.to_search_engine(),
            Some(SearchEngine::Brave)
        );
        assert_eq!(
            EngineFlag::Bing.to_search_engine(),
            Some(SearchEngine::Bing)
        );
        assert_eq!(
            EngineFlag::Google.to_search_engine(),
            Some(SearchEngine::Google)
        );
    }
}
