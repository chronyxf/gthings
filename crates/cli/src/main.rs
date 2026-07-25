use std::sync::Arc;

use clap::Parser;
use gthings_cdp::{detect, CdpError, Session};
use gthings_search::{follow, search, BatchProcessor};

#[derive(Parser)]
#[command(name = "gthings", version, about = "Browser automation and web research toolkit", disable_help_subcommand = true)]
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
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Port from `GTHINGS_CDP_PORT` env var (default 9222).
fn port() -> u16 {
    std::env::var("GTHINGS_CDP_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(9222)
}

/// Print a machine-readable error JSON to stderr.
fn print_error(code: &str, detail: &str, hint: &str) {
    let err = serde_json::json!({
        "error": code,
        "detail": detail,
        "hint": hint,
    });
    eprintln!("{}", err);
}

/// Detect browser → connect → return Session.
async fn connect() -> Result<Session, i32> {
    let p = port();

    // `detect` internally checks GTHINGS_CDP_WS_URL first (fast path),
    // then probes the CDP port via HTTP /json/version, /json, /json/list,
    // and finally DevToolsActivePort file scan.
    let browser = detect(p).await.map_err(|_| {
        print_error(
            "BROWSER_NOT_FOUND",
            &format!("No browser found on port {p}"),
            "Open Dia or Chrome with --remote-debugging-port=9222",
        );
        1
    })?;

    tracing::info!("Connecting to browser at {}", browser.ws_url);

    Session::connect(&browser.ws_url)
        .await
        .map_err(|e| {
            print_error(
                "CONNECTION_FAILED",
                &e.to_string(),
                "Verify WebSocket URL is accessible",
            );
            1
        })
}

/// Map common CDP errors to machine-readable error JSON.
fn on_cdp_error(e: &CdpError) {
    match e {
        CdpError::NavigationTimeout { .. } => {
            print_error(
                "NAVIGATION_TIMEOUT",
                &e.to_string(),
                "Check network connectivity or URL",
            );
        }
        _ => {
            print_error(
                "SEARCH_FAILED",
                &e.to_string(),
                "Retry with different arguments",
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Entrypoint
// ---------------------------------------------------------------------------

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
        Command::Status { json } => cmd_status(json).await,
        Command::Search { query, count, json } => cmd_search(&query, count, json).await,
        Command::Follow { url, max_chars, json } => cmd_follow(&url, max_chars, json).await,
        Command::Batch {
            queries,
            count,
            follow,
            max_chars,
            json,
        } => cmd_batch(queries, count, follow, max_chars, json).await,

    };
    std::process::exit(code);
}

// ---------------------------------------------------------------------------
// Subcommand implementations
// ---------------------------------------------------------------------------

/// Status: detect only, no connection needed.
async fn cmd_status(json: bool) -> i32 {
    match detect(port()).await {
        Ok(browser) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "status": "running",
                        "ws_url": browser.ws_url,
                        "browser": browser.browser,
                        "version": browser.version,
                    })
                );
            } else {
                println!("Browser: {} {}", browser.browser, browser.version);
                println!("WebSocket URL: {}", browser.ws_url);
            }
            0
        }
        Err(CdpError::BrowserNotFound { .. }) => {
            if json {
                let output = serde_json::json!({
                    "status": "stopped"
                });
                println!("{output}");
            } else {
                println!("Browser is NOT running");
            }
            0
        }
        Err(e) => {
            print_error(
                "DETECT_FAILED",
                &e.to_string(),
                "Check browser debugging port",
            );
            1
        }
    }
}

/// Search: detect → connect → create tab → search → close tab → disconnect.
async fn cmd_search(query: &str, count: usize, json: bool) -> i32 {
    let session = match connect().await {
        Ok(s) => s,
        Err(c) => return c,
    };

    let tab = match session.create_tab("about:blank").await {
        Ok(t) => t,
        Err(e) => {
            print_error("TAB_CREATE_FAILED", &e.to_string(), "Check browser connection");
            if let Err(e) = session.disconnect().await {
                tracing::warn!("disconnect failed: {e}");
            }
            return 1;
        }
    };

    let results = match search(&session, &tab, query, count).await {
        Ok(r) => r,
        Err(e) => {
            if let Err(e) = session.close_tab(tab).await {
                tracing::warn!("close_tab failed: {e}");
            }
            if let Err(e) = session.disconnect().await {
                tracing::warn!("disconnect failed: {e}");
            }
            on_cdp_error(&e);
            return 1;
        }
    };

    if let Err(e) = session.close_tab(tab).await {
        tracing::warn!("close_tab failed: {e}");
    }
    if let Err(e) = session.disconnect().await {
        tracing::warn!("disconnect failed: {e}");
    }

    if json {
        let output = serde_json::to_string(&results).unwrap_or_else(|e| {
            tracing::error!("serialize output failed: {e}");
            String::new()
        });
        println!("{}", output);
    } else {
        for r in &results {
            println!("#{} {} — {}", r.position, r.title, r.url);
            if !r.snippet.is_empty() {
                println!("  {}", r.snippet);
            }
        }
    }
    0
}

/// Follow: detect → connect → create tab → follow → close tab → disconnect.
async fn cmd_follow(url: &str, max_chars: usize, json: bool) -> i32 {
    let session = match connect().await {
        Ok(s) => s,
        Err(c) => return c,
    };

    let tab = match session.create_tab("about:blank").await {
        Ok(t) => t,
        Err(e) => {
            print_error("TAB_CREATE_FAILED", &e.to_string(), "Check browser connection");
            if let Err(e) = session.disconnect().await {
                tracing::warn!("disconnect failed: {e}");
            }
            return 1;
        }
    };

    let result = match follow(&session, &tab, url, max_chars).await {
        Ok(r) => r,
        Err(e) => {
            if let Err(e) = session.close_tab(tab).await {
                tracing::warn!("close_tab failed: {e}");
            }
            if let Err(e) = session.disconnect().await {
                tracing::warn!("disconnect failed: {e}");
            }
            on_cdp_error(&e);
            return 1;
        }
    };

    if let Err(e) = session.close_tab(tab).await {
        tracing::warn!("close_tab failed: {e}");
    }
    if let Err(e) = session.disconnect().await {
        tracing::warn!("disconnect failed: {e}");
    }

    if json {
        let output = serde_json::to_string(&result).unwrap_or_else(|e| {
            tracing::error!("serialize output failed: {e}");
            String::new()
        });
        println!("{}", output);
    } else {
        println!("Title: {}", result.title);
        println!("URL: {}", result.url);
        if result.truncated {
            println!("Content (truncated to {max_chars} chars):");
        } else {
            println!("Content:");
        }
        println!("{}", result.content);
    }
    0
}

/// Batch: detect → connect → batch → disconnect.
///
/// BatchProcessor::search creates and closes tabs internally, so we only
/// manage the session lifecycle at this level.
async fn cmd_batch(
    queries: Vec<String>,
    count: usize,
    do_follow: bool,
    max_chars: usize,
    json: bool,
) -> i32 {
    let session = match connect().await {
        Ok(s) => s,
        Err(c) => return c,
    };

    let arc_session = Arc::new(session);

    let all_results =
        match BatchProcessor::search(Arc::clone(&arc_session), &queries, count, do_follow, max_chars)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                print_error(
                    "BATCH_FAILED",
                    &e.to_string(),
                    "Retry with fewer queries or longer timeout",
                );
                return 1;
            }
        };

    // Clean disconnect when possible (unique reference).
    if let Ok(s) = Arc::try_unwrap(arc_session) {
        if let Err(e) = s.disconnect().await {
            tracing::warn!("disconnect failed: {e}");
        }
    }

    if json {
        let output = serde_json::to_string(&all_results).unwrap_or_else(|e| {
            tracing::error!("serialize output failed: {e}");
            String::new()
        });
        println!("{}", output);
    } else {
        for (i, results) in all_results.iter().enumerate() {
            println!("Query #{}:", i + 1);
            for r in results {
                println!("  #{} {} — {}", r.position, r.title, r.url);
                if !r.snippet.is_empty() {
                    println!("    {}", r.snippet);
                }
            }
        }
    }
    0
}


