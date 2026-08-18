//! `gthings update` — update binary and install/refresh skill files for opencode integration.

use std::fs;
use std::path::PathBuf;
use tokio::process::Command;

/// Embedded skill content — compiled into the binary at build time.
const SKILL_MAIN: &str = include_str!("../../resources/skills/opencode/gthings/SKILL.md");
const SKILL_AGENT: &str = include_str!("../../resources/skills/agents/gthings/SKILL.md");
const REF_COMMANDS: &str =
    include_str!("../../resources/skills/agents/gthings/reference/commands.md");
const REF_QUALITY: &str =
    include_str!("../../resources/skills/agents/gthings/reference/quality.md");

/// Resolve the user's home directory once from `$HOME`.
fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

/// Determine the opencode config root (`~/.config/opencode`), derived from
/// [`home_dir`].
fn opencode_dir() -> Option<PathBuf> {
    Some(home_dir()?.join(".config").join("opencode"))
}

/// Update gthings binary and install/refresh skill files.
///
/// `update` has no output flags and is human-only: every message goes to
/// **stderr**, keeping stdout clean for machine consumers. When
/// `GTHINGS_UPDATE_DISABLED` is set the command is a no-op (used by the Go
/// integration contract to prevent background mutations of the environment).
pub(crate) async fn cmd_update() -> i32 {
    if gthings_common::config::Config::load().update_disabled {
        eprintln!("gthings update disabled (GTHINGS_UPDATE_DISABLED set); skipping.");
        return 0;
    }

    // Step 1: install the latest binary.
    eprintln!("Updating gthings from cargo...");

    let status = Command::new("cargo")
        .args(["install", "gthings"])
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .await;

    match status {
        Ok(s) if s.success() => {}
        Ok(s) => {
            eprintln!("Warning: `cargo install gthings` exited with code {s}");
            // Continue anyway — maybe skill-install is still useful.
        }
        Err(e) => {
            eprintln!("Warning: could not run `cargo install gthings`: {e}");
            // Do not abort; proceed to install skill files.
        }
    }

    // Step 2: install the skill files.
    eprintln!("Installing skill files...");

    // Resolve the home directory once; both the opencode and agent skill
    // destinations derive from it.
    let Some(home_dir) = home_dir() else {
        eprintln!("Warning: could not determine home directory; skill files not installed.");
        eprintln!("Set $HOME to install skill files under ~/.config/opencode/.");
        return 0;
    };
    let base = opencode_dir().unwrap_or_else(|| home_dir.join(".config").join("opencode"));

    // Destination paths
    let skill_dir = base.join("skills").join("gthings");
    // `base` is `~/.config/opencode`; the agent skills live under `~/.agents`,
    // so derive them from the shared home directory.
    let agent_dir = home_dir.join(".agents").join("skills").join("gthings");
    let ref_dir = agent_dir.join("reference");

    // Create directories (log warnings on failure — writes will also surface issues)
    fs::create_dir_all(&skill_dir).unwrap_or_else(|e| {
        tracing::warn!(error = %e, path = %skill_dir.display(), "failed to create skill directory");
    });
    fs::create_dir_all(&agent_dir).unwrap_or_else(|e| {
        tracing::warn!(error = %e, path = %agent_dir.display(), "failed to create agent directory");
    });
    fs::create_dir_all(&ref_dir).unwrap_or_else(|e| {
        tracing::warn!(error = %e, path = %ref_dir.display(), "failed to create reference directory");
    });

    // Write files, collecting individual results
    let writes: [(&str, &str, PathBuf); 4] = [
        (
            "SKILL.md (opencode)",
            SKILL_MAIN,
            skill_dir.join("SKILL.md"),
        ),
        ("SKILL.md (agent)", SKILL_AGENT, agent_dir.join("SKILL.md")),
        (
            "reference/commands.md",
            REF_COMMANDS,
            ref_dir.join("commands.md"),
        ),
        (
            "reference/quality.md",
            REF_QUALITY,
            ref_dir.join("quality.md"),
        ),
    ];

    let mut ok = true;
    for (label, content, path) in &writes {
        if let Err(e) = fs::write(path, content) {
            eprintln!(
                "Warning: could not write {} to {}: {e}",
                label,
                path.display()
            );
            ok = false;
        } else {
            eprintln!("  ✓ {} → {}", label, path.display());
        }
    }

    if ok {
        eprintln!("Done! gthings updated to latest version.");
    } else {
        eprintln!("Done (with warnings — see above).");
    }

    0
}
