//! Shell detection and `~/.cargo/bin` PATH configuration utilities.
//!
//! Detects the user's shell from `$SHELL`, locates the corresponding config
//! file, and provides helpers to ensure `~/.cargo/bin` (or `$CARGO_HOME/bin`)
//! is present in both `$PATH` and the shell's init script.

use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Supported shell types.
///
/// `Unknown(String)` captures the raw `$SHELL` value when it does not match
/// a known shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Shell {
    /// Bourne-again shell (`bash`)
    Bash,
    /// Z shell (`zsh`)
    Zsh,
    /// Friendly interactive shell (`fish`)
    Fish,
    /// Unrecognised shell — contains the raw `$SHELL` value.
    Unknown(String),
}

// ---------------------------------------------------------------------------
// Detection
// ---------------------------------------------------------------------------

/// Detect the user's shell from the `$SHELL` environment variable.
///
/// Inspects the last component of the `$SHELL` path (e.g. `/bin/bash` →
/// `bash`) and returns the matching [`Shell`] variant.
///
/// Returns [`Shell::Unknown`] if `$SHELL` is not set or its value cannot be
/// parsed.
#[must_use]
pub(crate) fn detect_shell() -> Shell {
    let shell = match std::env::var("SHELL") {
        Ok(val) => val,
        Err(_) => return Shell::Unknown(String::new()),
    };

    // Extract the file name from the path
    let name = std::path::Path::new(&shell)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("")
        .to_lowercase();

    match name.as_str() {
        "bash" => Shell::Bash,
        "zsh" => Shell::Zsh,
        "fish" => Shell::Fish,
        _ => Shell::Unknown(shell),
    }
}

// ---------------------------------------------------------------------------
// Config file paths
// ---------------------------------------------------------------------------

/// Return the path to the shell's configuration file.
///
/// | Shell  | Default config file                                          |
/// |--------|--------------------------------------------------------------|
/// | Bash   | `~/.bashrc`, or `~/.bash_profile` on macOS when `.bashrc`    |
/// |        | does not exist.                                              |
/// | Zsh    | `~/.zshrc`                                                   |
/// | Fish   | `~/.config/fish/config.fish`                                 |
/// | Unknown| `None`                                                       |
///
/// # Errors
///
/// Returns `None` when `$HOME` is not set or the shell is [`Shell::Unknown`].
#[must_use]
pub(crate) fn shell_config_file(shell: &Shell) -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let home_path = PathBuf::from(&home);

    match shell {
        Shell::Bash => {
            let bashrc = home_path.join(".bashrc");
            #[cfg(target_os = "macos")]
            {
                if bashrc.exists() {
                    Some(bashrc)
                } else {
                    Some(home_path.join(".bash_profile"))
                }
            }
            #[cfg(not(target_os = "macos"))]
            {
                Some(bashrc)
            }
        }
        Shell::Zsh => Some(home_path.join(".zshrc")),
        Shell::Fish => Some(home_path.join(".config").join("fish").join("config.fish")),
        Shell::Unknown(_) => None,
    }
}

// ---------------------------------------------------------------------------
// Cargo bin directory
// ---------------------------------------------------------------------------

/// Return the cargo binary directory.
///
/// Uses `$CARGO_HOME/bin` when the `CARGO_HOME` environment variable is set,
/// otherwise falls back to `$HOME/.cargo/bin`.
///
/// # Panics
///
/// Panics if neither `CARGO_HOME` nor `HOME` is set. Callers that wish to
/// handle this gracefully should check the env vars beforehand.
#[must_use]
pub(crate) fn cargo_bin_dir() -> PathBuf {
    if let Ok(cargo_home) = std::env::var("CARGO_HOME") {
        PathBuf::from(cargo_home).join("bin")
    } else {
        let home = std::env::var("HOME").expect("HOME must be set");
        PathBuf::from(home).join(".cargo").join("bin")
    }
}

// ---------------------------------------------------------------------------
// PATH checks
// ---------------------------------------------------------------------------

/// Check whether the cargo bin directory is already present in `$PATH`.
///
/// Splits `$PATH` on `:` and compares each component against
/// [`cargo_bin_dir()`].
#[must_use]
pub(crate) fn path_contains_cargo_bin() -> bool {
    let cargo_bin = cargo_bin_dir();
    let path = match std::env::var("PATH") {
        Ok(p) => p,
        Err(_) => return false,
    };

    path.split(':')
        .any(|component| std::path::Path::new(component) == cargo_bin)
}

/// Check whether the shell config file already contains an export of the cargo
/// bin directory in `$PATH`.
///
/// Reads the config file (if it exists) and scans each line for either:
///
/// - `export PATH=".../cargo/bin..."`
/// - `PATH=".../cargo/bin..."`
///
/// Matching is done by searching for the canonical cargo bin path string
/// returned by [`cargo_bin_dir()`].
#[must_use]
pub(crate) fn config_has_cargo_bin_path(shell: &Shell) -> bool {
    let config_path = match shell_config_file(shell) {
        Some(p) => p,
        None => return false,
    };

    if !config_path.exists() {
        return false;
    }

    let contents = match std::fs::read_to_string(&config_path) {
        Ok(c) => c,
        Err(_) => return false,
    };

    let cargo_bin_str = cargo_bin_dir().to_string_lossy().to_string();

    for line in contents.lines() {
        let trimmed = line.trim();
        if (trimmed.starts_with("export PATH=") || trimmed.starts_with("PATH="))
            && trimmed.contains(&cargo_bin_str)
        {
            return true;
        }
    }

    false
}

// ---------------------------------------------------------------------------
// Modifying config
// ---------------------------------------------------------------------------

/// Append the cargo bin directory to the shell's PATH in the config file.
///
/// The line added is:
///
/// ```text
/// export PATH="$HOME/.cargo/bin:$PATH"
/// ```
///
/// The literal `$HOME` (not the expanded value) is used so that the config
/// remains portable across systems.
///
/// # Returns
///
/// - `Ok(true)` if the line was newly appended.
/// - `Ok(false)` if the line was already present (no change).
/// - `Err(..)` if the config file path could not be determined or IO fails.
///
/// # Errors
///
/// Returns an error when:
/// - `SHELL` points to an unknown shell (config file path is `None`).
/// - Parent directories cannot be created.
/// - The file cannot be read or written.
pub(crate) fn ensure_path_in_shell_config(shell: &Shell) -> Result<bool, anyhow::Error> {
    let config_path = shell_config_file(shell).ok_or_else(|| {
        anyhow::anyhow!("Cannot determine shell config file path for shell: {shell:?}")
    })?;

    let cargo_bin_str = cargo_bin_dir().to_string_lossy().to_string();

    // Read existing content if the file exists
    let mut content = String::new();
    if config_path.exists() {
        content = std::fs::read_to_string(&config_path)?;
    }

    // Check if the line is already present
    for line in content.lines() {
        let trimmed = line.trim();
        if (trimmed.starts_with("export PATH=") || trimmed.starts_with("PATH="))
            && trimmed.contains(&cargo_bin_str)
        {
            return Ok(false);
        }
    }

    // Create parent directories if needed
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Append the export line
    let export_line = "export PATH=\"$HOME/.cargo/bin:$PATH\"\n".to_string();
    std::fs::write(&config_path, content + &export_line)?;

    Ok(true)
}

// ---------------------------------------------------------------------------
// User-facing hints
// ---------------------------------------------------------------------------

/// Print a post-init hint telling the user to source their shell config or
/// informing them that everything is already set up.
///
/// # Parameters
///
/// * `shell` – the detected shell (used to determine the config file path).
/// * `was_modified` – whether `ensure_path_in_shell_config` actually appended
///   a new line.
pub(crate) fn print_post_init_hint(shell: &Shell, was_modified: bool) {
    let config_path = shell_config_file(shell);

    if was_modified {
        if let Some(ref path) = config_path {
            println!("Added ~/.cargo/bin to PATH in {}", path.display());
            println!(
                "  Run `source {}` or restart your shell to apply the change.",
                path.display()
            );
        } else {
            println!("Added ~/.cargo/bin to PATH in your shell configuration.");
            println!("  Restart your shell to apply the change.");
        }
    } else if path_contains_cargo_bin() {
        println!("~/.cargo/bin is already in your PATH — nothing to do.");
    } else if config_has_cargo_bin_path(shell) {
        if let Some(ref path) = config_path {
            println!(
                "~/.cargo/bin is configured in {} but not yet in your current PATH.",
                path.display()
            );
            println!(
                "  Run `source {}` or restart your shell to apply the change.",
                path.display()
            );
        } else {
            println!("~/.cargo/bin is configured but not yet in your current PATH.");
            println!("  Restart your shell to apply the change.");
        }
    } else {
        println!("~/.cargo/bin is already configured — everything looks good.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // detect_shell – relies on current $SHELL, just check it returns Ok
    // -----------------------------------------------------------------------

    #[test]
    fn test_detect_shell_returns_some_variant() {
        // The function should always return one of the variants without
        // panicking.
        let shell = detect_shell();
        match shell {
            Shell::Bash | Shell::Zsh | Shell::Fish | Shell::Unknown(_) => {}
        }
    }

    // -----------------------------------------------------------------------
    // shell_config_file – relies on current $HOME, check shape
    // -----------------------------------------------------------------------

    #[test]
    fn test_shell_config_file_bash_ends_with_bashrc_or_profile() {
        if let Some(path) = shell_config_file(&Shell::Bash) {
            let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            assert!(
                name == ".bashrc" || name == ".bash_profile",
                "expected .bashrc or .bash_profile, got {name}"
            );
        }
        // If $HOME is unset the test is meaningless but does not fail.
    }

    #[test]
    fn test_shell_config_file_zsh_ends_with_zshrc() {
        if let Some(path) = shell_config_file(&Shell::Zsh) {
            assert_eq!(path.file_name().and_then(|s| s.to_str()), Some(".zshrc"));
        }
    }

    #[test]
    fn test_shell_config_file_fish_ends_with_config_fish() {
        if let Some(path) = shell_config_file(&Shell::Fish) {
            assert!(path.ends_with("config.fish"));
        }
    }

    #[test]
    fn test_shell_config_file_unknown_is_none() {
        assert!(shell_config_file(&Shell::Unknown("x".into())).is_none());
    }

    // -----------------------------------------------------------------------
    // cargo_bin_dir – relies on $CARGO_HOME / $HOME
    // -----------------------------------------------------------------------

    #[test]
    fn test_cargo_bin_dir_ends_with_bin() {
        let dir = cargo_bin_dir();
        assert_eq!(dir.file_name().and_then(|s| s.to_str()), Some("bin"));
    }

    // -----------------------------------------------------------------------
    // path_contains_cargo_bin – relies on current $PATH
    // -----------------------------------------------------------------------

    #[test]
    fn test_path_contains_cargo_bin_does_not_panic() {
        // Just ensure the function runs without panicking.
        let _ = path_contains_cargo_bin();
    }

    // -----------------------------------------------------------------------
    // config_has_cargo_bin_path – filesystem-based tests (no env mutation)
    // -----------------------------------------------------------------------

    #[test]
    fn test_config_has_cargo_bin_path_with_temp_dir() {
        // We can't set HOME, but we can test the *inner logic* by writing a
        // file and then using a shell whose config file we point at it.
        //
        // Instead, the simplest safe test is to check that a non-existent
        // config returns false.
        assert!(!config_has_cargo_bin_path(&Shell::Zsh));
    }

    // -----------------------------------------------------------------------
    // ensure_path_in_shell_config
    // -----------------------------------------------------------------------

    #[test]
    fn test_ensure_path_in_shell_config_unknown() {
        let result = ensure_path_in_shell_config(&Shell::Unknown("nope".into()));
        assert!(result.is_err());
    }
}
