mod browser_commands;
mod follow_commands;
mod pdf_commands;
mod search_commands;

use clap::Parser;
use std::io::Write;
use std::time::SystemTime;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

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
    /// Take a screenshot of a web page (requires daemon)
    Screenshot {
        /// URL to capture
        url: String,
        /// Output file path
        #[arg(short, long, default_value = "screenshot.png")]
        output: std::path::PathBuf,
        /// Output as JSON instead of writing file
        #[arg(long)]
        json: bool,
    },
    /// Scrape content from a web page using CSS selectors (requires daemon)
    Scrape {
        /// URL to scrape
        url: String,
        /// CSS selector
        #[arg(short, long, default_value = "body")]
        selector: String,
        /// Attribute to extract (default: innerText)
        #[arg(short, long)]
        attribute: Option<String>,
        /// Output as JSON Lines
        #[arg(long)]
        json: bool,
    },
    /// Browser automation (CDP) — requires daemon
    Browser(BrowserArgs),
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

#[derive(clap::Args)]
struct BrowserArgs {
    #[command(subcommand)]
    command: BrowserCommand,
}

#[derive(clap::Subcommand)]
enum BrowserCommand {
    Status,
    Start {
        port: Option<u16>,
    },
    Stop,
    Logs {
        follow: bool,
    },
    Call {
        method: String,
        params: String,
    },
    Eval {
        expression: String,
    },
    Navigate {
        url: String,
    },
    Wait {
        method: String,
        session: String,
        #[arg(long, default_value = "30000")]
        timeout: u64,
    },
}

/// Fetch the daemon context from the UDS socket for trace enrichment.
///
/// Returns `None` if the daemon is not reachable (commands that need the
/// daemon will fail separately; this is best-effort enrichment).
async fn fetch_daemon_context() -> Option<serde_json::Value> {
    let socket_path = std::env::var("GTHINGS_DAEMON_SOCKET")
        .unwrap_or_else(|_| "/tmp/gthings-daemon.sock".to_string());

    let stream = match UnixStream::connect(&socket_path).await {
        Ok(s) => s,
        Err(_) => return None,
    };

    let (reader, mut writer) = stream.into_split();
    let request = serde_json::json!({"id": 1, "method": "get_context", "params": null});
    let mut buf = serde_json::to_vec(&request).ok()?;
    buf.push(b'\n');
    writer.write_all(&buf).await.ok()?;
    writer.shutdown().await.ok()?;

    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    reader.read_line(&mut line).await.ok()?;

    let response: serde_json::Value = serde_json::from_str(&line).ok()?;
    if response["ok"].as_bool().unwrap_or(false) {
        response["result"]
            .as_object()
            .map(|_| response["result"].clone())
    } else {
        None
    }
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
    let trace_path = cli.trace.clone();
    // Fetch daemon context for trace enrichment (best-effort)
    let daemon_context = fetch_daemon_context().await;

    let cmd_start = std::time::Instant::now();
    let (tool_name, tool_args) = command_metadata(&cli.command);

    let result = match &cli.command {
        Command::Search(args) => match &args.command {
            SearchCommand::Query { query, count } => {
                search_commands::handle_search_query(&config, query, *count, cli.json).await
            }
            SearchCommand::Batch { queries, count } => {
                search_commands::handle_search_batch(&config, queries, *count, cli.json).await
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
                follow_commands::handle_follow_url(&config, url, selector, *offset, *max, cli.json)
                    .await
            }
            FollowCommand::Batch {
                urls,
                selector,
                offset,
                max,
            } => {
                follow_commands::handle_follow_batch(
                    &config, urls, selector, *offset, *max, cli.json,
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
        Command::Screenshot { url, output, json } => {
            let config = common::config::GthingsConfig::default();
            browser_commands::handle_screenshot(&config, url, output, *json).await?;
            Ok(())
        }
        Command::Scrape {
            url,
            selector,
            attribute,
            json,
        } => {
            let config = common::config::GthingsConfig::default();
            browser_commands::handle_scrape(&config, url, selector, attribute.as_deref(), *json)
                .await?;
            Ok(())
        }
        Command::Browser(args) => {
            let config = common::config::GthingsConfig::default();
            match &args.command {
                BrowserCommand::Status => browser_commands::handle_browser_status(&config).await?,
                BrowserCommand::Start { port } => {
                    browser_commands::handle_browser_start(&config, *port).await?
                }
                BrowserCommand::Stop => browser_commands::handle_browser_stop(&config).await?,
                BrowserCommand::Logs { follow } => {
                    browser_commands::handle_browser_logs(&config, *follow).await?
                }
                BrowserCommand::Call { method, params } => {
                    browser_commands::handle_browser_call(&config, method, params).await?
                }
                BrowserCommand::Eval { expression } => {
                    browser_commands::handle_browser_eval(&config, expression).await?
                }
                BrowserCommand::Navigate { url } => {
                    browser_commands::handle_browser_navigate(&config, url).await?
                }
                BrowserCommand::Wait {
                    method,
                    session,
                    timeout,
                } => {
                    browser_commands::handle_browser_wait(&config, method, session, *timeout)
                        .await?
                }
            };
            Ok(())
        }
    };

    // Telemetry capture
    let cmd_duration_ms = cmd_start.elapsed().as_millis() as u64;
    let exit_code = if result.is_ok() { 0 } else { 1 };

    if let Some(ref path) = trace_path {
        let record = serde_json::json!({
            "ts": trace_timestamp(),
            "session": session_id,
            "tool": tool_name,
            "args": tool_args,
            "duration_ms": cmd_duration_ms,
            "exit": exit_code,
        });
        write_trace(path, &record, &daemon_context);
    }

    result
}

/// Write a trace record to the trace file (if configured).
/// Format: JSONL — one JSON object per line.
fn write_trace(trace_path: &str, record: &serde_json::Value, context: &Option<serde_json::Value>) {
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(trace_path)
    {
        let mut record = record.clone();
        if let Some(ctx) = context {
            record["context"] = ctx.clone();
        }
        let mut line = serde_json::to_string(&record).unwrap_or_default();
        line.push('\n');
        let _ = file.write_all(line.as_bytes());
    }
}

/// Return a Unix timestamp with nanosecond precision for telemetry.
fn trace_timestamp() -> String {
    let dur = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}.{:09}", dur.as_secs(), dur.subsec_nanos())
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
        Command::Screenshot {
            url,
            output: _,
            json,
        } => ("screenshot", serde_json::json!({"url": url, "json": json})),
        Command::Scrape {
            url,
            selector,
            attribute: _,
            json: _,
        } => (
            "scrape",
            serde_json::json!({"url": url, "selector": selector}),
        ),
        Command::Browser(args) => match &args.command {
            BrowserCommand::Status => ("browser_status", serde_json::json!({})),
            BrowserCommand::Start { port } => ("browser_start", serde_json::json!({"port": port})),
            BrowserCommand::Stop => ("browser_stop", serde_json::json!({})),
            BrowserCommand::Logs { follow } => {
                ("browser_logs", serde_json::json!({"follow": follow}))
            }
            BrowserCommand::Call { method, params: _ } => {
                ("browser_call", serde_json::json!({"method": method}))
            }
            BrowserCommand::Eval { expression } => (
                "browser_eval",
                serde_json::json!({"expression": expression}),
            ),
            BrowserCommand::Navigate { url } => {
                ("browser_navigate", serde_json::json!({"url": url}))
            }
            BrowserCommand::Wait {
                method,
                session: _,
                timeout,
            } => (
                "browser_wait",
                serde_json::json!({"method": method, "timeout": timeout}),
            ),
        },
    }
}
