mod follow_commands;
mod pdf_commands;
mod search_commands;

use clap::Parser;
use gthings_common::trace::TraceWriter;
use include_dir::{Dir, include_dir};
use std::fs;
use std::path::Path;
use std::time::SystemTime;

static SKILLS_DIR: Dir = include_dir!("$CARGO_MANIFEST_DIR/resources/skills");

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
    /// Update gthings to the latest version
    Update,
    /// Manage gthings skills (install to opencode or agents)
    Skill(SkillArgs),
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

#[derive(clap::Args)]
pub struct SkillArgs {
    #[command(subcommand)]
    pub command: SkillCommand,
}

#[derive(clap::Subcommand)]
pub enum SkillCommand {
    /// Install gthings skills
    Add {
        /// Install skills to opencode directory (~/.config/opencode/skills/gthings-*)
        #[arg(long)]
        opencode: bool,
        /// Install skills to agents directory (~/.agents/skills/gthings/)
        #[arg(long)]
        agents: bool,
        /// Install to both opencode and agents
        #[arg(long)]
        all: bool,
    },
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let cli = Cli::parse();

    let filter = tracing_subscriber::EnvFilter::try_new(&cli.log_level)
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let config = gthings_common::config::GthingsConfig::from_env();

    let session_id = format!(
        "ses_{:x}",
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );

    // Initialize TraceWriter if --trace is provided
    let mut trace_writer = cli
        .trace
        .as_ref()
        .and_then(|path| TraceWriter::new(path).ok());

    let cmd_start = std::time::Instant::now();
    let (tool_name, tool_args) = command_metadata(&cli.command);

    // Get a borrow to pass through to handlers
    let trace = trace_writer.as_mut();

    let result = match &cli.command {
        Command::Search(args) => match &args.command {
            SearchCommand::Query { query, count } => {
                search_commands::handle_search_query(&config, query, *count, cli.json, trace).await
            }
            SearchCommand::Batch { queries, count } => {
                search_commands::handle_search_batch(&config, queries, *count, cli.json, trace)
                    .await
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
                    trace,
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
                follow_commands::handle_follow_url(
                    &config, url, selector, *offset, *max, cli.json, trace,
                )
                .await
            }
            FollowCommand::Batch {
                urls,
                selector,
                offset,
                max,
            } => {
                follow_commands::handle_follow_batch(
                    &config, urls, selector, *offset, *max, cli.json, trace,
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
        Command::Update => cmd_update().await,
        Command::Skill(args) => cmd_skill(args).await,
    };

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
    let browser = gthings_cdp::Browser::launch()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to start browser: {e}"))?;
    let _conn = browser
        .connect()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to connect: {e}"))?;
    let pid = browser.pid().await.unwrap_or(0);
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

/// Stop the persistent browser.
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

    if pid > 0 {
        let _ = std::process::Command::new("kill")
            .arg(pid.to_string())
            .status();
    }

    std::fs::remove_file(&state_path)?;

    if json {
        println!("{}", serde_json::json!({"status": "stopped", "pid": pid}));
    } else {
        println!("Browser stopped (pid={})", pid);
    }
    Ok(())
}

/// Show browser status.
async fn handle_browser_status(json: bool) -> Result<(), anyhow::Error> {
    let existing = gthings_cdp::Browser::find_existing().await;
    if let Some(browser) = existing {
        let pid = browser.pid().await.unwrap_or(0);
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

async fn cmd_update() -> anyhow::Result<()> {
    println!("Updating gthings...");
    let status = std::process::Command::new("cargo")
        .args(["install", "gthings"])
        .status()
        .map_err(|e| anyhow::anyhow!("Failed to run cargo install: {}", e))?;
    if !status.success() {
        anyhow::bail!(
            "cargo install gthings failed with exit code: {:?}",
            status.code()
        );
    }
    println!("gthings updated to latest version.");
    println!("  Run 'gthings skill add --all' to update skill files.");
    Ok(())
}

async fn cmd_skill(args: &SkillArgs) -> anyhow::Result<()> {
    match &args.command {
        SkillCommand::Add {
            opencode,
            agents,
            all,
        } => {
            let do_opencode = *opencode || *all;
            let do_agents = *agents || *all;

            if !do_opencode && !do_agents {
                anyhow::bail!("Specify --opencode, --agents, or --all");
            }

            let home = std::env::var("HOME")
                .map_err(|_| anyhow::anyhow!("HOME environment variable not set"))?;

            if do_agents {
                let dest = Path::new(&home)
                    .join(".agents")
                    .join("skills")
                    .join("gthings");
                if let Some(skill_dir) = SKILLS_DIR.get_dir("agents/gthings") {
                    copy_embedded_dir(skill_dir, &dest)?;
                    let count = count_files(skill_dir);
                    println!(
                        "Installed {} files to agents skill: {}",
                        count,
                        dest.display()
                    );
                } else {
                    anyhow::bail!("Embedded agents skill directory not found");
                }
            }

            if do_opencode {
                let dest = Path::new(&home)
                    .join(".config")
                    .join("opencode")
                    .join("skills");
                if let Some(opencode_dir) = SKILLS_DIR.get_dir("opencode") {
                    for skill_subdir in opencode_dir.dirs() {
                        let skill_name = skill_subdir
                            .path()
                            .file_name()
                            .ok_or_else(|| anyhow::anyhow!("Invalid skill directory name"))?;
                        let skill_dest = dest.join(skill_name);
                        copy_embedded_dir(skill_subdir, &skill_dest)?;
                        let count = count_files(skill_subdir);
                        println!(
                            "  - Installed {} files to opencode skill: {}",
                            count,
                            skill_dest.display()
                        );
                    }
                } else {
                    anyhow::bail!("Embedded opencode skills directory not found");
                }
            }

            println!("Skill installation complete.");
            Ok(())
        }
    }
}

fn copy_embedded_dir(dir: &Dir, dest: &Path) -> anyhow::Result<()> {
    for file in dir.files() {
        let relative = file
            .path()
            .strip_prefix(dir.path())
            .map_err(|_| anyhow::anyhow!("Failed to compute relative path"))?;
        let dest_path = dest.join(relative);
        if let Some(parent) = dest_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&dest_path, file.contents())?;
    }
    for subdir in dir.dirs() {
        let relative = subdir
            .path()
            .strip_prefix(dir.path())
            .map_err(|_| anyhow::anyhow!("Failed to compute relative path"))?;
        let subdest = dest.join(relative);
        fs::create_dir_all(&subdest)?;
        copy_embedded_dir(subdir, &subdest)?;
    }
    Ok(())
}

fn count_files(dir: &Dir) -> usize {
    let mut count = dir.files().count();
    for subdir in dir.dirs() {
        count += count_files(subdir);
    }
    count
}

/// Extract command metadata for telemetry.
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
        Command::Update => ("update", serde_json::json!({})),
        Command::Skill(args) => match &args.command {
            SkillCommand::Add { .. } => ("skill_add", serde_json::json!({})),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skills_dir_has_agents_gthings() {
        let dir = SKILLS_DIR.get_dir("agents/gthings");
        assert!(dir.is_some(), "agents/gthings dir should exist");
    }

    #[test]
    fn test_skills_dir_has_opencode() {
        let dir = SKILLS_DIR.get_dir("opencode");
        assert!(dir.is_some(), "opencode dir should exist");
    }

    #[test]
    fn test_agents_skill_md_exists() {
        let dir = SKILLS_DIR.get_dir("agents/gthings").unwrap();
        let has_skill_md = dir.files().any(|f| f.path().ends_with("SKILL.md"));
        assert!(has_skill_md, "agents/gthings should contain SKILL.md");
    }

    #[test]
    fn test_agents_has_reference_files() {
        let dir = SKILLS_DIR.get_dir("agents/gthings").unwrap();
        let count = count_files(dir);
        assert!(
            count >= 3,
            "agents/gthings should have at least 3 files, got {}",
            count
        );
    }

    #[test]
    fn test_opencode_has_gthings() {
        let dir = SKILLS_DIR.get_dir("opencode").unwrap();
        let has_gthings = dir.dirs().any(|d| d.path().ends_with("gthings"));
        assert!(has_gthings, "opencode should contain gthings dir");
    }

    #[test]
    fn test_opencode_has_only_gthings() {
        let dir = SKILLS_DIR.get_dir("opencode").unwrap();
        let skill_names: Vec<_> = dir
            .dirs()
            .map(|d| d.path().file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(
            skill_names,
            vec!["gthings"],
            "opencode should only contain 'gthings' skill, got: {:?}",
            skill_names
        );
    }

    #[test]
    fn test_copy_embedded_dir_to_temp() {
        use std::fs;
        let tmp = std::env::temp_dir().join(format!("gthings_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        let skill_dir = SKILLS_DIR.get_dir("agents/gthings").unwrap();
        copy_embedded_dir(skill_dir, &tmp).unwrap();

        assert!(tmp.join("SKILL.md").exists(), "SKILL.md should be copied");
        assert!(
            tmp.join("reference").join("commands.md").exists(),
            "commands.md should be copied"
        );
        assert!(
            tmp.join("reference").join("quality.md").exists(),
            "quality.md should be copied"
        );

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_copy_embedded_dir_count_matches() {
        use std::fs;
        let tmp = std::env::temp_dir().join(format!("gthings_test_cnt_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        let skill_dir = SKILLS_DIR.get_dir("agents/gthings").unwrap();
        let count_before = count_files(skill_dir);
        copy_embedded_dir(skill_dir, &tmp).unwrap();

        let count_after = count_dir_files(&tmp);
        assert_eq!(
            count_before, count_after,
            "count_files should match actual copied files"
        );

        let _ = fs::remove_dir_all(&tmp);
    }

    fn count_dir_files(path: &std::path::Path) -> usize {
        let mut count = 0;
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    count += count_dir_files(&entry.path());
                } else {
                    count += 1;
                }
            }
        }
        count
    }

    #[test]
    fn test_opencode_gthings_has_yaml_frontmatter() {
        let dir = SKILLS_DIR.get_dir("opencode/gthings").unwrap();
        let skill_md = dir.files().find(|f| f.path().ends_with("SKILL.md"));
        assert!(skill_md.is_some(), "opencode/gthings should have SKILL.md");
        let content = String::from_utf8_lossy(skill_md.unwrap().contents());
        assert!(
            content.starts_with("---"),
            "opencode/gthings/SKILL.md should start with YAML frontmatter"
        );
        assert!(
            content.contains("name: gthings"),
            "should have name: gthings"
        );
        assert!(
            content.contains("description:"),
            "should have description field"
        );
    }
}
