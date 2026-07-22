use std::fs::File;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::prelude::*;

use browser_daemon::{CdpDaemon, DaemonConfig};

#[derive(Parser)]
#[command(name = "browser-daemon")]
struct Cli {
    #[command(subcommand)]
    command: DaemonCommand,
}

#[derive(Subcommand)]
enum DaemonCommand {
    /// Start the CDP daemon (foreground)
    Start {
        #[arg(long, default_value = "9222")]
        port: u16,
        #[arg(long)]
        chrome_path: Option<String>,
        #[arg(long)]
        profile_dir: Option<String>,
    },
    /// Check daemon status (reads PID file)
    Status,
    /// Stop the daemon gracefully
    Stop,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        DaemonCommand::Start {
            port,
            chrome_path,
            profile_dir,
        } => {
            let config = DaemonConfig {
                cdp_port: port,
                chrome_path: chrome_path.map(PathBuf::from),
                profile_dir: profile_dir.map(PathBuf::from),
                ..Default::default()
            };
            // ── Initialize tracing ──────────────────────────────────────────
            // Write JSONL trace to the daemon log file for diagnostics.
            // The CLI's --trace captures command telemetry; this captures
            // daemon-internal events (discovery, connection, errors).
            let log_path = config.log_path.clone();
            if let Ok(log_file) = File::create(&log_path) {
                let file_layer = tracing_subscriber::fmt::layer()
                    .json()
                    .with_writer(Arc::new(log_file))
                    .with_target(true)
                    .with_thread_ids(true);
                let filter = tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

                tracing_subscriber::registry()
                    .with(filter)
                    .with(file_layer)
                    .init();

                tracing::info!(path = %log_path.display(), "daemon logging initialized");
            } else {
                eprintln!(
                    "Warning: Could not create daemon log at {}",
                    log_path.display()
                );
                // Fallback to stderr logging
                tracing_subscriber::fmt()
                    .with_env_filter(
                        tracing_subscriber::EnvFilter::try_from_default_env()
                            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
                    )
                    .init();
            }

            let daemon = CdpDaemon::new(config);
            daemon.run().await?;
        }

        DaemonCommand::Status => {
            let pid_path = PathBuf::from("/tmp/gthings-daemon.pid");
            let status = match std::fs::read_to_string(&pid_path) {
                Ok(content) => {
                    let pid: u32 = content.trim().parse().unwrap_or(0);
                    let running = std::process::Command::new("kill")
                        .arg("-0")
                        .arg(pid.to_string())
                        .status()
                        .map(|s| s.success())
                        .unwrap_or(false);
                    serde_json::json!({
                        "running": running,
                        "pid": pid,
                        "cdp_port": null,
                        "chrome_connected": false,
                        "uptime_secs": null,
                        "version": null,
                    })
                }
                Err(_) => serde_json::json!({
                    "running": false,
                    "pid": null,
                    "cdp_port": null,
                    "chrome_connected": false,
                    "uptime_secs": null,
                    "version": null,
                }),
            };
            println!("{}", serde_json::to_string_pretty(&status)?);
        }

        DaemonCommand::Stop => {
            let pid_path = PathBuf::from("/tmp/gthings-daemon.pid");
            let pid = match std::fs::read_to_string(&pid_path) {
                Ok(c) => c
                    .trim()
                    .parse::<u32>()
                    .map_err(|_| anyhow::anyhow!("Invalid PID in {}", pid_path.display()))?,
                Err(_) => {
                    anyhow::bail!("Daemon not running (no PID file at {})", pid_path.display())
                }
            };

            std::process::Command::new("kill")
                .arg(pid.to_string())
                .status()?;

            // Clean up stale PID file.
            let _ = std::fs::remove_file(&pid_path);

            println!(r#"{{"ok":true,"stopped":true}}"#);
        }
    }

    Ok(())
}
