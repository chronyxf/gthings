use clap::Parser;

mod commands;

#[derive(Parser)]
#[command(
    name = "gthings",
    version,
    about = "Browser automation and web research toolkit",
    disable_help_subcommand = true
)]
enum Command {
    /// Search Google and return results (JSON array)
    Search {
        query: String,
        #[arg(long, default_value = "5")]
        count: usize,
        #[arg(long)]
        json: bool,
    },
    /// Follow/extract page content (JSON object with title/content/url)
    Follow {
        url: String,
        #[arg(long, default_value = "15000")]
        max_chars: usize,
        #[arg(long, default_value = "0")]
        offset: usize,
        #[arg(long)]
        json: bool,
    },
    /// Batch search (JSON array of result arrays)
    Batch {
        queries: Vec<String>,
        #[arg(long, default_value = "5")]
        count: usize,
        #[arg(long)]
        follow: bool,
        #[arg(long, default_value = "15000")]
        max_chars: usize,
        #[arg(long)]
        json: bool,
    },
    /// Check browser connection (JSON with status/running/stopped)
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Extract content from any URL (auto-detects PDF, GitHub, arXiv, web)
    Extract {
        url: String,
        #[arg(long, default_value = "15000")]
        max_chars: usize,
        #[arg(long, default_value = "0")]
        offset: usize,
        #[arg(long)]
        json: bool,
    },
    /// Harvest: search → dedup → rank → follow (full research pipeline)
    Harvest {
        queries: Vec<String>,
        #[arg(long, default_value = "url")]
        dedup: String,
        #[arg(long, default_value = "composite")]
        rank: String,
        #[arg(long, default_value = "8")]
        follow_top: usize,
        #[arg(long, default_value = "15000")]
        max_chars: usize,
        #[arg(long)]
        json: bool,
        #[arg(long, default_value = "20")]
        warn_tabs: usize,
    },
    /// Extract text from a PDF
    Pdf {
        #[command(subcommand)]
        command: PdfCommand,
    },
}

#[derive(clap::Subcommand)]
enum PdfCommand {
    /// Extract text from PDF at URL
    Url {
        url: String,
        #[arg(long, default_value = "15000")]
        max_chars: usize,
        #[arg(long, default_value = "0")]
        offset: usize,
        #[arg(long)]
        json: bool,
    },
    /// Extract text from local PDF file
    File {
        path: std::path::PathBuf,
        #[arg(long, default_value = "15000")]
        max_chars: usize,
        #[arg(long, default_value = "0")]
        offset: usize,
        #[arg(long)]
        json: bool,
    },
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .without_time()
        .init();

    let command = Command::parse();
    let code = match command {
        Command::Status { json } => commands::cmd_status(json).await,
        Command::Search { query, count, json } => commands::cmd_search(&query, count, json).await,
        Command::Follow {
            url,
            max_chars,
            offset,
            json,
        } => commands::cmd_follow(&url, max_chars, offset, json).await,
        Command::Batch {
            queries,
            count,
            follow,
            max_chars,
            json,
        } => commands::cmd_batch(queries, count, follow, max_chars, json).await,
        Command::Harvest {
            queries,
            dedup,
            rank,
            follow_top,
            max_chars,
            json,
            warn_tabs,
        } => {
            commands::cmd_harvest(queries, dedup, rank, follow_top, max_chars, json, warn_tabs)
                .await
        }
        Command::Extract {
            url,
            max_chars,
            offset,
            json,
        } => commands::cmd_extract(&url, max_chars, offset, json).await,
        Command::Pdf { command } => match command {
            PdfCommand::Url {
                url,
                max_chars,
                offset,
                json,
            } => commands::cmd_pdf_url(&url, max_chars, offset, json).await,
            PdfCommand::File {
                path,
                max_chars,
                offset,
                json,
            } => commands::cmd_pdf_file(&path, max_chars, offset, json).await,
        },
    };
    std::process::exit(code);
}
