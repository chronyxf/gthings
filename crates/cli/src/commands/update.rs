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

/// Determine the opencode config root (`~/.config/opencode`).
fn opencode_dir() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".config").join("opencode"))
}

/// Update gthings binary and install/refresh skill files.
pub(crate) async fn cmd_update() -> i32 {
    // ── Step 1: cargo install ──────────────────────────────────────────
    println!("Updating gthings from cargo...");

    let status = Command::new("cargo")
        .args(["install", "gthings"])
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .await;

    match status {
        Ok(s) if s.success() => {}
        Ok(s) => {
            eprintln!("Warning: `cargo install gthings` exited with code {}", s);
            // Continue anyway — maybe skill-install is still useful.
        }
        Err(e) => {
            eprintln!("Warning: could not run `cargo install gthings`: {e}");
            // Do not abort; proceed to install skill files.
        }
    }

    // ── Step 2: install skill files ────────────────────────────────────
    println!("Installing skill files...");

    let Some(base) = opencode_dir() else {
        eprintln!("Warning: could not determine home directory; skill files not installed.");
        eprintln!("Set $HOME to install skill files under ~/.config/opencode/.");
        return 0;
    };

    let Some(home) = std::env::var("HOME").ok() else {
        eprintln!("Warning: could not determine home directory; skill files not installed.");
        return 0;
    };

    // Destination paths
    let skill_dir = base.join("skills").join("gthings");
    let agent_dir = PathBuf::from(&home)
        .join(".agents")
        .join("skills")
        .join("gthings");
    let ref_dir = agent_dir.join("reference");

    // Create directories (ignore errors — let writes fail below for specific messages)
    let _ = fs::create_dir_all(&skill_dir);
    let _ = fs::create_dir_all(&agent_dir);
    let _ = fs::create_dir_all(&ref_dir);

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
            println!("  ✓ {} → {}", label, path.display());
        }
    }

    if ok {
        println!("Done! gthings updated to latest version.");
    } else {
        println!("Done (with warnings — see above).");
    }

    0
}
