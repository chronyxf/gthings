use clap::Parser;
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
        /// Strategy: simple (default), parallel, harvest
        #[arg(long, value_enum, default_value = "simple")]
        strategy: SearchStrategy,
        /// Extract content from result URLs (parallel/harvest)
        #[arg(long)]
        extract_results: bool,
        /// Max chars per extracted page (parallel/harvest)
        #[arg(long, default_value = "15000")]
        max_chars: usize,
        /// Dedup strategy for harvest
        #[arg(long, default_value = "url")]
        dedup: String,
        /// Rank strategy for harvest
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
        #[arg(long, default_value = "15000")]
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
        #[arg(long, default_value = "15000")]
        max_chars: usize,
        #[arg(long, default_value = "0")]
        offset: usize,
    },
    /// Extract text from local PDF file
    PdfFile {
        #[command(flatten)]
        universal: commands::UniversalFlags,
        path: std::path::PathBuf,
        #[arg(long, default_value = "15000")]
        max_chars: usize,
        #[arg(long, default_value = "0")]
        offset: usize,
    },
}

#[derive(clap::ValueEnum, Clone, Debug)]
enum SearchStrategy {
    Simple,
    Parallel,
    Harvest,
}

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
        30,
        commands::cmd_search(universal, &queries[0], count),
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
) -> i32 {
    run_with_timeout(
        "parallel search",
        60,
        commands::cmd_batch(universal, queries, count, extract_results, max_chars),
    )
    .await
    .unwrap_or_else(|e| e)
}

async fn handle_search_harvest(
    universal: &mut commands::UniversalFlags,
    queries: Vec<String>,
    dedup: String,
    rank: String,
    follow_top: usize,
    max_chars: usize,
    warn_tabs: usize,
) -> i32 {
    run_with_timeout(
        "harvest",
        120,
        commands::cmd_harvest(
            universal, queries, dedup, rank, follow_top, max_chars, warn_tabs,
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
            extract_results,
            max_chars,
            dedup,
            rank,
            follow_top,
            warn_tabs,
        } => {
            universal.merge_from(&cli.universal);
            match strategy {
                SearchStrategy::Simple => handle_search_simple(universal, queries, count).await,
                SearchStrategy::Parallel => {
                    handle_search_parallel(universal, queries, count, extract_results, max_chars)
                        .await
                }
                SearchStrategy::Harvest => {
                    handle_search_harvest(
                        universal, queries, dedup, rank, follow_top, max_chars, warn_tabs,
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
