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
        Command::Status { ref mut universal } => {
            universal.merge_from(&cli.universal);
            match tokio::time::timeout(Duration::from_secs(10), commands::cmd_status(universal))
                .await
            {
                Ok(result) => result,
                Err(_) => {
                    eprintln!("gthings: status command timed out after 10s");
                    1
                }
            }
        }
        Command::Update => {
            match tokio::time::timeout(Duration::from_secs(60), commands::cmd_update()).await {
                Ok(result) => result,
                Err(_) => {
                    eprintln!("gthings: update command timed out after 60s");
                    1
                }
            }
        }
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
                SearchStrategy::Simple => {
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
                        1
                    } else {
                        match tokio::time::timeout(
                            Duration::from_secs(30),
                            commands::cmd_search(universal, &queries[0], count),
                        )
                        .await
                        {
                            Ok(result) => result,
                            Err(_) => {
                                eprintln!("gthings: search command timed out after 30s");
                                1
                            }
                        }
                    }
                }
                SearchStrategy::Parallel => {
                    match tokio::time::timeout(
                        Duration::from_secs(60),
                        commands::cmd_batch(universal, queries, count, extract_results, max_chars),
                    )
                    .await
                    {
                        Ok(result) => result,
                        Err(_) => {
                            eprintln!("gthings: parallel search command timed out after 60s");
                            1
                        }
                    }
                }
                SearchStrategy::Harvest => {
                    match tokio::time::timeout(
                        Duration::from_secs(120),
                        commands::cmd_harvest(
                            universal, queries, dedup, rank, follow_top, max_chars, warn_tabs,
                        ),
                    )
                    .await
                    {
                        Ok(result) => result,
                        Err(_) => {
                            eprintln!("gthings: harvest command timed out after 120s");
                            1
                        }
                    }
                }
            }
        }

        Command::Extract {
            ref mut universal,
            url,
            max_chars,
            offset,
        } => {
            universal.merge_from(&cli.universal);
            match tokio::time::timeout(
                Duration::from_secs(30),
                commands::cmd_extract(universal, &url, max_chars, offset),
            )
            .await
            {
                Ok(result) => result,
                Err(_) => {
                    eprintln!("gthings: extract command timed out after 30s");
                    1
                }
            }
        }
        Command::Ax {
            ref mut universal,
            url,
            max_nodes,
        } => {
            universal.merge_from(&cli.universal);
            let max_nodes = if max_nodes == 0 {
                None
            } else {
                Some(max_nodes)
            };
            match tokio::time::timeout(
                Duration::from_secs(30),
                commands::cmd_ax(universal, &url, max_nodes),
            )
            .await
            {
                Ok(result) => result,
                Err(_) => {
                    eprintln!("gthings: ax command timed out after 30s");
                    1
                }
            }
        }
        Command::PdfUrl {
            ref mut universal,
            url,
            max_chars,
            offset,
        } => {
            universal.merge_from(&cli.universal);
            match tokio::time::timeout(
                Duration::from_secs(30),
                commands::cmd_pdf_url(universal, &url, max_chars, offset),
            )
            .await
            {
                Ok(result) => result,
                Err(_) => {
                    eprintln!("gthings: pdf url command timed out after 30s");
                    1
                }
            }
        }
        Command::PdfFile {
            ref mut universal,
            path,
            max_chars,
            offset,
        } => {
            universal.merge_from(&cli.universal);
            match tokio::time::timeout(
                Duration::from_secs(15),
                commands::cmd_pdf_file(universal, &path, max_chars, offset),
            )
            .await
            {
                Ok(result) => result,
                Err(_) => {
                    eprintln!("gthings: pdf file command timed out after 15s");
                    1
                }
            }
        }
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
