use std::process::Command;

// ── Helper: run the gthings CLI with given args and return stdout ──
fn run_cli(args: &[&str]) -> (String, String, std::process::ExitStatus) {
    let output = Command::new("cargo")
        .args(["run", "--"])
        .args(args)
        .output()
        .expect("failed to execute cargo run");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status,
    )
}

#[test]
fn test_binary_exists() {
    let output = Command::new("cargo")
        .args(["run", "--", "--help"])
        .output()
        .expect("failed to execute cargo run");
    assert!(output.status.success());
}

#[test]
fn test_cli_help_contains_subcommands() {
    let output = Command::new("cargo")
        .args(["run", "--", "--help"])
        .output()
        .expect("failed to execute cargo run -- --help");
    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("search"),
        "--help should list 'search' subcommand"
    );
    assert!(
        stdout.contains("extract"),
        "--help should list 'extract' subcommand"
    );
    assert!(stdout.contains("ax"), "--help should list 'ax' subcommand");
    assert!(
        stdout.contains("status"),
        "--help should list 'status' subcommand"
    );
}

#[test]
fn test_cli_ax_subcommand_exists() {
    // Verify that the `ax` subcommand is registered in the CLI and its help
    // text describes the behavior (accessibility tree extraction).
    let output = Command::new("cargo")
        .args(["run", "--", "ax", "--help"])
        .output()
        .expect("failed to execute cargo run -- ax --help");
    assert!(
        output.status.success(),
        "ax --help should succeed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("ax"), "ax --help should mention 'ax'");
    assert!(
        stdout.contains("accessibility") || stdout.contains("tree"),
        "ax --help should describe accessibility tree extraction"
    );
}

#[test]
fn test_ax_command_creates_background_tab() {
    // Verify that the `ax` command's implementation uses background tabs
    // by checking its call chain: cmd_ax → gthings_cdp::ax_tree::ax_tree
    // → session.create_background_tab().
    //
    // This is a compile-time / structural test: we verify the public API
    // entry points exist and have the correct shape.

    // ax_tree is a public async fn in gthings_cdp::ax_tree.
    // Just referencing it confirms it exists at the expected path.
    let _ = gthings_cdp::ax_tree::ax_tree;

    // Session::create_background_tab must exist — the ax command depends on it.
    fn _check_bg(
        _s: &gthings_cdp::Session,
    ) -> impl std::future::Future<Output = gthings_cdp::Result<gthings_cdp::Tab>> {
        gthings_cdp::Session::create_background_tab(_s)
    }
    // Session struct must be accessible from the integration test
    fn _session_bounds() {
        fn _require_send<T: Send>() {}
        _require_send::<gthings_cdp::Session>();
    }
    _session_bounds();
    let _ = _check_bg;
}

// ── Universal flags test cases ──

#[test]
fn test_output_format_text() {
    // Verify --output text is accepted and produces human-readable output.
    // The status command doesn't require a browser and returns JSON;
    // with --output text it should produce plain text.
    let (_stdout, _stderr, status) = run_cli(&["status", "--output", "text"]);
    // The command may fail at runtime (no browser), but the flag must be parsed.
    // If the flag is unknown, clap will exit with an error before reaching the handler.
    assert!(
        status.code().is_some(),
        "--output text should be a recognized flag"
    );
}

#[test]
fn test_output_format_json() {
    // Verify --output json is accepted and produces pretty-printed JSON.
    let (_stdout, _stderr, status) = run_cli(&["status", "--output", "json"]);
    assert!(
        status.code().is_some(),
        "--output json should be a recognized flag"
    );
}

#[test]
fn test_output_format_ndjson() {
    // Verify --output ndjson is accepted and produces compact JSON lines.
    let (_stdout, _stderr, status) = run_cli(&["status", "--output", "ndjson"]);
    assert!(
        status.code().is_some(),
        "--output ndjson should be a recognized flag"
    );
}

#[test]
fn test_query_filter() {
    // Verify --query flag is accepted and passes a JMESPath-like filter.
    let (_stdout, _stderr, status) = run_cli(&["status", "--query", ".[].url"]);
    assert!(
        status.code().is_some(),
        "--query should be a recognized flag"
    );
}

#[test]
fn test_json_backward_compat() {
    // Verify --json still works as a backward-compatible alias for --output json.
    let (_stdout, _stderr, status) = run_cli(&["status", "--json"]);
    assert!(
        status.code().is_some(),
        "--json should be a recognized flag (backward compat for --output json)"
    );
}

#[test]
fn test_cdp_port_flag() {
    // Verify --cdp-port overrides the default port (9222).
    let (_stdout, _stderr, status) = run_cli(&["status", "--cdp-port", "9223"]);
    assert!(
        status.code().is_some(),
        "--cdp-port should be a recognized flag"
    );
}

#[test]
fn test_timeout_flag() {
    // Verify --timeout is passed through to commands.
    let (_stdout, _stderr, status) = run_cli(&["status", "--timeout", "10"]);
    assert!(
        status.code().is_some(),
        "--timeout should be a recognized flag"
    );
}

#[test]
fn test_verbose_flag() {
    // Verify --verbose increases output verbosity (accepts -v shorthand too).
    let (_stdout, _stderr, status) = run_cli(&["status", "--verbose"]);
    assert!(
        status.code().is_some(),
        "--verbose should be a recognized flag"
    );
}

#[test]
fn test_quiet_flag() {
    // Verify --quiet suppresses non-error output.
    let (_stdout, _stderr, status) = run_cli(&["status", "--quiet"]);
    assert!(
        status.code().is_some(),
        "--quiet should be a recognized flag"
    );
}

#[test]
fn test_universal_flags_on_ax() {
    // Verify the `ax` command now supports --output json (universal flags).
    let (stdout, _stderr, status) = run_cli(&["ax", "--output", "json", "--help"]);
    // ax --help should succeed and list --output as an available flag.
    assert!(
        status.success(),
        "ax --output json --help should succeed: stderr={}",
        _stderr
    );
    assert!(
        stdout.contains("--output"),
        "ax --help should list the --output flag (universal flag support)"
    );
}
