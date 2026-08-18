use clap::Parser;
use gthings_common::config::Config;
use tracing::Instrument;

mod args;
mod commands;
mod describe;
mod timeout;
mod util;

pub(crate) use args::{Cli, Command, EngineFlag, SearchArgs, SearchStrategy};

use crate::describe::handle_describe;
use crate::timeout::{command_timeout, run_with_timeout};

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let root_span = init_tracing(cli.universal.tracing_level());

    // Log panics to stderr instead of silent failure
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        eprintln!("gthings: panic: {info}");
        default_hook(info);
    }));

    // Load the shared config once (env vars are immutable for the process
    // lifetime), so per-command timeout resolution doesn't re-parse env on
    // every dispatch.
    let config = Config::load();

    // `gthings serve` owns its own signal handling: the daemon drains in-flight
    // jobs and closes every live tab on SIGTERM/SIGINT via ServeHandle::shutdown.
    // Racing the generic shutdown-signal arm below would drop the daemon mid-
    // drain (orphaning tabs), so the daemon subcommand bypasses the select.
    if matches!(&cli.command, Command::Serve) {
        let code = dispatch(cli, &config).instrument(root_span).await;
        std::process::exit(code);
    }

    // Race the in-flight command against an OS shutdown signal. On signal we
    // exit with the conventional code (SIGTERM → 143, SIGINT → 130) without an
    // envelope, dropping the dispatch future so TabGuard can close any tabs.
    tokio::select! {
        code = dispatch(cli, &config).instrument(root_span) => {
            std::process::exit(code);
        }
        signal = shutdown_signal() => {
            std::process::exit(signal);
        }
    }
}

/// Route the parsed CLI to its command implementation, returning the process
/// exit code. Timeout wrapping and flag resolution live here (no separate
/// handler layer): every command is dispatched directly and wrapped in its
/// per-command timeout.
async fn dispatch(cli: Cli, config: &Config) -> i32 {
    let universal = &cli.universal;
    match cli.command {
        Command::Status => timed(config, "status", commands::cmd_status(universal)).await,
        Command::Health => timed(config, "health", commands::cmd_health(universal)).await,
        Command::Update => timed(config, "update", commands::cmd_update()).await,
        Command::Serve => commands::cmd_serve().await,
        Command::Config => commands::cmd_config(universal),
        Command::Search {
            queries,
            count,
            strategy,
            engine,
            extract_results,
            max_chars,
            rank,
            follow_top,
            warn_tabs,
        } => {
            let timeout_name = match &strategy {
                SearchStrategy::Simple => "search",
                SearchStrategy::Parallel => "parallel_search",
                SearchStrategy::Harvest => "harvest",
            };
            timed(
                config,
                timeout_name,
                commands::cmd_search(
                    universal,
                    SearchArgs {
                        queries,
                        count,
                        strategy,
                        engine,
                        extract_results,
                        max_chars,
                        rank,
                        follow_top,
                        warn_tabs,
                    },
                ),
            )
            .await
        }
        Command::Extract {
            url,
            max_chars,
            offset,
        } => {
            timed(
                config,
                "extract",
                commands::cmd_extract(universal, &url, max_chars, offset),
            )
            .await
        }
        Command::Ax { url, max_nodes } => {
            let max_nodes = if max_nodes == 0 {
                None
            } else {
                Some(max_nodes)
            };
            timed(config, "ax", commands::cmd_ax(universal, &url, max_nodes)).await
        }
        Command::PdfUrl {
            url,
            max_chars,
            offset,
        } => {
            timed(
                config,
                "pdf_url",
                commands::cmd_pdf_url(universal, &url, max_chars, offset),
            )
            .await
        }
        Command::PdfFile {
            path,
            max_chars,
            offset,
        } => {
            timed(
                config,
                "pdf_file",
                commands::cmd_pdf_file(universal, &path, max_chars, offset),
            )
            .await
        }
        Command::Describe => handle_describe(universal),
    }
}

/// Run `fut` under the per-command timeout resolved from `config`
/// (`GTHINGS_<CMD>_TIMEOUT` override, else the built-in default from
/// [`command_timeout`]).
async fn timed<F: std::future::Future<Output = i32>>(config: &Config, cmd: &str, fut: F) -> i32 {
    let default = command_timeout(cmd);
    let secs = config.command_timeout(cmd).unwrap_or(default.as_secs());
    run_with_timeout(cmd, secs, fut).await
}

/// Wait for a shutdown signal and return the conventional exit code:
/// SIGTERM → 143, SIGINT → 130 (128 + signal number).
async fn shutdown_signal() -> i32 {
    use tokio::signal::unix::{SignalKind, signal};
    let mut sigterm = signal(SignalKind::terminate()).expect("failed to register SIGTERM handler");
    let mut sigint = signal(SignalKind::interrupt()).expect("failed to register SIGINT handler");
    tokio::select! {
        _ = sigterm.recv() => 143,
        _ = sigint.recv() => 130,
    }
}

/// Initialize the tracing subscriber, using the given log level string.
/// Returns the root span, seeded with the TRACEPARENT trace id so every
/// instrumented handler is correlated with the calling job.
fn init_tracing(level: &str) -> tracing::Span {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(level)),
        )
        .with_writer(std::io::stderr)
        .init();
    tracing::info_span!("gthings", trace_id = %gthings_common::telemetry::trace_id())
}
