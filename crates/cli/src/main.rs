mod follow_commands;
mod pdf_commands;
mod search_commands;

use clap::Parser;
use common::trace::TraceWriter;
use std::time::SystemTime;

#[derive(clap::Parser)]
#[command(
    name = "gthings",
    version,
    about = "Browser automation and web research toolkit"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// Output as JSON Lines
    #[arg(global = true, long)]
    json: bool,

    /// Log level
    #[arg(global = true, long, default_value = "info")]
    log_level: String,

    /// Trace file path — write structured JSONL telemetry for every command
    #[arg(global = true, long)]
    trace: Option<String>,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Search the web
    Search(SearchArgs),
    /// Follow/extract page content
    Follow(FollowArgs),
    /// PDF text extraction
    Pdf(PdfArgs),
    /// Browser lifecycle management
    #[command(name = "browser", hide = true)]
    Browser(BrowserArgs),
}

#[derive(clap::Args)]
struct BrowserArgs {
    #[command(subcommand)]
    command: BrowserCommand,
}

#[derive(clap::Subcommand)]
enum BrowserCommand {
    /// Start the persistent browser (auto-started on first use)
    Start,
    /// Stop the persistent browser
    Stop,
    /// Show browser status
    Status,
}

#[derive(clap::Args)]
struct SearchArgs {
    #[command(subcommand)]
    command: SearchCommand,
}

#[derive(clap::Subcommand)]
enum SearchCommand {
    /// Single Google search
    Query {
        query: String,
        #[arg(long, default_value = "10")]
        count: usize,
    },
    /// Batch search multiple queries
    Batch {
        queries: Vec<String>,
        #[arg(long, default_value = "5")]
        count: usize,
    },
    /// Two-phase: search then follow top results
    Harvest {
        queries: Vec<String>,
        #[arg(long, default_value = "5")]
        count: usize,
        #[arg(long)]
        max: Option<usize>,
        /// Max concurrent search tabs (default: from env or 3)
        #[arg(long)]
        concurrency: Option<usize>,
        /// Max concurrent follow tabs (default: from env or 3)
        #[arg(long, name = "follow-concurrency")]
        follow_concurrency: Option<usize>,
    },
}

#[derive(clap::Args)]
struct FollowArgs {
    #[command(subcommand)]
    command: FollowCommand,
}

#[derive(clap::Subcommand)]
enum FollowCommand {
    /// Single URL extraction
    Url {
        url: String,
        #[arg(long, default_value = "article,main,[role=main]")]
        selector: String,
        #[arg(long, default_value = "0")]
        offset: usize,
        #[arg(long, default_value = "15000")]
        max: usize,
    },
    /// Batch multi-page extraction
    Batch {
        urls: Vec<String>,
        #[arg(long, default_value = "article,main,[role=main]")]
        selector: String,
        #[arg(long, default_value = "0")]
        offset: usize,
        #[arg(long, default_value = "15000")]
        max: usize,
    },
}

#[derive(clap::Args)]
struct PdfArgs {
    #[command(subcommand)]
    command: PdfCommand,
}

#[derive(clap::Subcommand)]
enum PdfCommand {
    /// Extract text from PDF at URL
    Url { url: String },
    /// Extract text from local PDF file
    File { path: std::path::PathBuf },
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let cli = Cli::parse();

    // Initialize tracing
    let filter = tracing_subscriber::EnvFilter::try_new(&cli.log_level)
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    // Load config from environment
    let config = common::config::GthingsConfig::from_env();

    // Telemetry setup
    let session_id = format!(
        "ses_{:x}",
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );

    // Initialize TraceWriter if --trace is provided
    let mut trace_writer = cli.trace.as_ref().and_then(|path| {
        TraceWriter::new(path).ok()
    });

    let cmd_start = std::time::Instant::now();
    let (tool_name, tool_args) = command_metadata(&cli.command);

    // Get a borrow to pass through to handlers
    let mut trace = trace_writer.as_mut();

    let result = match &cli.command {
        Command::Search(args) => match &args.command {
            SearchCommand::Query { query, count } => {
                search_commands::handle_search_query(&config, query, *count, cli.json, trace.as_deref_mut()).await
            }
            SearchCommand::Batch { queries, count } => {
                search_commands::handle_search_batch(&config, queries, *count, cli.json, trace.as_deref_mut()).await
            }
            SearchCommand::Harvest {
                queries,
                count,
                max,
                concurrency,
                follow_concurrency,
            } => {
                search_commands::handle_search_harvest(
                    &config,
                    queries,
                    *count,
                    *max,
                    *concurrency,
                    *follow_concurrency,
                    cli.json,
                    trace.as_deref_mut(),
                )
                .await
            }
        },
        Command::Follow(args) => match &args.command {
            FollowCommand::Url {
                url,
                selector,
                offset,
                max,
            } => {
                follow_commands::handle_follow_url(&config, url, selector, *offset, *max, cli.json, trace.as_deref_mut())
                    .await
            }
            FollowCommand::Batch {
                urls,
                selector,
                offset,
                max,
            } => {
                follow_commands::handle_follow_batch(
                    &config, urls, selector, *offset, *max, cli.json, trace.as_deref_mut(),
                )
                .await
            }
        },
        Command::Pdf(args) => match &args.command {
            PdfCommand::Url { url } => pdf_commands::handle_pdf_url(&config, url, cli.json).await,
            PdfCommand::File { path } => {
                pdf_commands::handle_pdf_file(&config, path, cli.json).await
            }
        },
        Command::Browser(args) => match &args.command {
            BrowserCommand::Start => handle_browser_start(cli.json).await,
            BrowserCommand::Stop => handle_browser_stop(cli.json).await,
            BrowserCommand::Status => handle_browser_status(cli.json).await,
        },
    };

    // Telemetry capture via TraceWriter
    let cmd_duration_ms = cmd_start.elapsed().as_millis() as u64;
    let exit_code = if result.is_ok() { 0 } else { 1 };

    let error_msg = if exit_code != 0 {
        result.as_ref().err().map(|e| e.to_string())
    } else {
        None
    };
    if let Some(ref mut t) = trace_writer {
        t.step(
            &session_id,
            0,
            tool_name,
            "command",
            None,
            cmd_duration_ms,
            Some(tool_args),
            Some(serde_json::json!({"exit": exit_code})),
            error_msg.as_deref(),
        );
    }

    result
}

// Browser lifecycle handlers

/// Path to the browser state file.
fn browser_state_path() -> std::path::PathBuf {
    let home = std::env::var("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("/tmp"));
    home.join(".gthings").join("browser.json")
}

/// Start the persistent browser.
async fn handle_browser_start(json: bool) -> Result<(), anyhow::Error> {
    let browser = cdp::Browser::launch()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to start browser: {e}"))?;
    let _conn = browser
        .connect()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to connect: {e}"))?;
    let pid = browser.pid().unwrap_or(0);
    if json {
        println!(
            "{}",
            serde_json::json!({
                "status": "started",
                "pid": pid,
                "ws_url": browser.ws_url(),
            })
        );
    } else {
        println!("Browser started (pid={})", pid);
        println!("WebSocket URL: {}", browser.ws_url());
    }
    Ok(())
}

/// Stop the persistent browser: kill process and remove state file.
async fn handle_browser_stop(json: bool) -> Result<(), anyhow::Error> {
    let state_path = browser_state_path();
    if !state_path.exists() {
        if json {
            println!("{}", serde_json::json!({"status": "not_running"}));
        } else {
            println!("No browser state found — browser is not running");
        }
        return Ok(());
    }
    let state_str = std::fs::read_to_string(&state_path)?;
    let state: serde_json::Value = serde_json::from_str(&state_str)?;
    let pid = state["pid"].as_u64().unwrap_or(0);

    // Kill the process
    if pid > 0 {
        let _ = std::process::Command::new("kill").arg(pid.to_string()).status();
    }

    // Remove state file
    std::fs::remove_file(&state_path)?;

    if json {
        println!("{}", serde_json::json!({"status": "stopped", "pid": pid}));
    } else {
        println!("Browser stopped (pid={})", pid);
    }
    Ok(())
}

/// Show browser status (running or stopped).
async fn handle_browser_status(json: bool) -> Result<(), anyhow::Error> {
    let existing = cdp::Browser::find_existing().await;
    if let Some(browser) = existing {
        let pid = browser.pid().unwrap_or(0);
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "status": "running",
                    "pid": pid,
                    "ws_url": browser.ws_url(),
                })
            );
        } else {
            println!("Browser is RUNNING (pid={})", pid);
            println!("WebSocket URL: {}", browser.ws_url());
        }
    } else {
        if json {
            println!("{}", serde_json::json!({"status": "stopped"}));
        } else {
            println!("Browser is NOT running");
        }
    }
    Ok(())
}

/// Extract command name and key arguments from the Command enum for telemetry.
fn command_metadata(cmd: &Command) -> (&'static str, serde_json::Value) {
    match cmd {
        Command::Search(args) => match &args.command {
            SearchCommand::Query { query, count } => (
                "search",
                serde_json::json!({"query": query, "count": count}),
            ),
            SearchCommand::Batch { queries, count } => (
                "search_batch",
                serde_json::json!({"queries_count": queries.len(), "count": count}),
            ),
            SearchCommand::Harvest {
                queries,
                count,
                max,
                concurrency,
                follow_concurrency,
            } => (
                "search_harvest",
                serde_json::json!({
                    "queries_count": queries.len(),
                    "count": count,
                    "max": max,
                    "concurrency": concurrency,
                    "follow_concurrency": follow_concurrency,
                }),
            ),
        },
        Command::Follow(args) => match &args.command {
            FollowCommand::Url {
                url,
                selector,
                offset: _,
                max,
            } => (
                "follow",
                serde_json::json!({"url": url, "selector": selector, "max": max}),
            ),
            FollowCommand::Batch {
                urls,
                selector: _,
                offset: _,
                max,
            } => (
                "follow_batch",
                serde_json::json!({"urls_count": urls.len(), "max": max}),
            ),
        },
        Command::Pdf(args) => match &args.command {
            PdfCommand::Url { url } => ("pdf_url", serde_json::json!({"url": url})),
            PdfCommand::File { path } => (
                "pdf_file",
                serde_json::json!({"path": format!("{}", path.display())}),
            ),
        },
        Command::Browser(args) => match &args.command {
            BrowserCommand::Start => ("browser_start", serde_json::json!({})),
            BrowserCommand::Stop => ("browser_stop", serde_json::json!({})),
            BrowserCommand::Status => ("browser_status", serde_json::json!({})),
        },
    }
}
