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
    let (_stdout, _stderr, status) = run_cli(&["--help"]);
    assert!(status.success());
}

#[test]
fn test_cli_help_contains_subcommands() {
    // Verify every subcommand is registered by inspecting the machine-parseable
    // describe guide (a parseable envelope), not help-text prose.
    let (stdout, stderr, status) = run_cli(&["describe", "--output", "json"]);
    assert!(
        status.success(),
        "describe --output json should succeed: stderr={stderr}"
    );
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("describe should emit valid JSON: {e}\nstdout={stdout}"));
    let subs = parsed["data"]["subcommands"]
        .as_object()
        .unwrap_or_else(|| panic!("describe data.subcommands should be an object: {stdout}"));
    for sub in [
        "search", "extract", "ax", "status", "health", "serve", "config",
    ] {
        assert!(
            subs.contains_key(sub),
            "describe guide should list the '{sub}' subcommand"
        );
    }
}

#[test]
fn test_cli_ax_subcommand_exists() {
    // Verify the `ax` subcommand is registered via the describe guide envelope.
    let (stdout, stderr, status) = run_cli(&["describe", "--output", "json"]);
    assert!(
        status.success(),
        "describe --output json should succeed: stderr={stderr}"
    );
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("describe should emit valid JSON: {e}\nstdout={stdout}"));
    assert!(
        parsed["data"]["subcommands"]
            .as_object()
            .map(|s| s.contains_key("ax"))
            .unwrap_or(false),
        "describe guide should list the 'ax' subcommand"
    );
}

// ── Universal flag tests: assert on parsed OUTPUT, not just exit status ──

#[test]
fn test_output_format_text() {
    // `--output text` renders the envelope as `key: value` lines rather than
    // JSON, so assert on the text content.
    let (stdout, stderr, status) = run_cli(&["status", "--output", "text"]);
    assert!(
        status.success(),
        "--output text should be a recognized flag: stderr={stderr}"
    );
    assert!(
        stdout.contains("status:"),
        "--output text should render the envelope status field: {stdout}"
    );
    assert!(
        stdout.contains("data:"),
        "--output text should render the envelope data field: {stdout}"
    );
}

#[test]
fn test_output_format_json() {
    // `--output json` must emit a parseable {status, data, error, trace_id}
    // envelope, regardless of whether a browser is running.
    let (stdout, stderr, status) = run_cli(&["status", "--output", "json"]);
    assert!(
        status.success(),
        "--output json should be a recognized flag: stderr={stderr}"
    );
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("--output json should emit valid JSON: {e}\nstdout={stdout}"));
    assert_eq!(parsed["status"], "ok");
    assert!(parsed["trace_id"].as_str().is_some_and(|t| !t.is_empty()));
    assert!(parsed["data"]["status"].is_string());
}

#[test]
fn test_output_format_ndjson() {
    // `--output nd-json` renders the envelope as a single compact JSON line.
    let (stdout, stderr, status) = run_cli(&["status", "--output", "nd-json"]);
    assert!(
        status.success(),
        "--output nd-json should be a recognized flag: stderr={stderr}"
    );
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!("--output nd-json should emit valid JSON: {e}\nstdout={stdout}")
    });
    assert_eq!(parsed["status"], "ok");
    assert!(parsed["trace_id"].as_str().is_some_and(|t| !t.is_empty()));
}

#[test]
fn test_query_filter() {
    // `--query` filters the envelope before formatting; `.data.status` extracts
    // the `"running"`/`"stopped"` readiness string.
    let (stdout, stderr, status) =
        run_cli(&["status", "--output", "json", "--query", ".data.status"]);
    assert!(
        status.success(),
        "--query should be a recognized flag: stderr={stderr}"
    );
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("--query output should be valid JSON: {e}\nstdout={stdout}"));
    assert!(
        parsed == "running" || parsed == "stopped",
        "--query .data.status should extract the readiness string: {stdout}"
    );
}

#[test]
fn test_json_backward_compat() {
    // `--json` is the backward-compatible alias for `--output json`.
    let (stdout, stderr, status) = run_cli(&["status", "--json"]);
    assert!(
        status.success(),
        "--json should be a recognized flag (backward compat for --output json): stderr={stderr}"
    );
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("--json should emit valid JSON: {e}\nstdout={stdout}"));
    assert_eq!(parsed["status"], "ok");
    assert!(parsed["trace_id"].as_str().is_some_and(|t| !t.is_empty()));
}

#[test]
fn test_cdp_port_flag() {
    // `--cdp-port` overrides the detection port; status still answers with a
    // valid envelope (running on that port, or stopped when nothing is there).
    let (stdout, stderr, status) = run_cli(&["status", "--output", "json", "--cdp-port", "9223"]);
    assert!(
        status.success(),
        "--cdp-port should be a recognized flag: stderr={stderr}"
    );
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("--cdp-port output should be valid JSON: {e}\nstdout={stdout}"));
    assert_eq!(parsed["status"], "ok");
    assert!(parsed["data"]["status"].is_string());
}

#[test]
fn test_timeout_flag() {
    // `--timeout` is accepted by every subcommand via the universal flags.
    let (stdout, stderr, status) = run_cli(&["status", "--output", "json", "--timeout", "10"]);
    assert!(
        status.success(),
        "--timeout should be a recognized flag: stderr={stderr}"
    );
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("--timeout output should be valid JSON: {e}\nstdout={stdout}"));
    assert_eq!(parsed["status"], "ok");
    assert!(parsed["trace_id"].as_str().is_some_and(|t| !t.is_empty()));
}

#[test]
fn test_verbose_flag() {
    // `--verbose` (and `-v`) increases verbosity; stdout still carries the
    // envelope.
    let (stdout, stderr, status) = run_cli(&["status", "--output", "json", "--verbose"]);
    assert!(
        status.success(),
        "--verbose should be a recognized flag: stderr={stderr}"
    );
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("--verbose output should be valid JSON: {e}\nstdout={stdout}"));
    assert_eq!(parsed["status"], "ok");
}

#[test]
fn test_quiet_flag() {
    // `--quiet` suppresses non-error output; the envelope still goes to stdout.
    let (stdout, stderr, status) = run_cli(&["status", "--output", "json", "--quiet"]);
    assert!(
        status.success(),
        "--quiet should be a recognized flag: stderr={stderr}"
    );
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("--quiet output should be valid JSON: {e}\nstdout={stdout}"));
    assert_eq!(parsed["status"], "ok");
    assert!(parsed["trace_id"].as_str().is_some_and(|t| !t.is_empty()));
}

#[test]
fn test_universal_flags_on_ax() {
    // Verify the `ax` command accepts the universal --output json flag: clap
    // must accept it before reaching the handler, so a successful exit status
    // proves the flag is recognized.
    let (_stdout, _stderr, status) = run_cli(&["ax", "--output", "json", "--help"]);
    assert!(
        status.success(),
        "ax --output json --help should succeed: stderr={_stderr}"
    );
}

#[test]
fn test_max_chars_flag_parses() {
    // The flag must parse a custom value; runtime may fail (network/PDF), but
    // clap must accept the argument before reaching the handler. A clap parse
    // error exits non-zero, so `success()` verifies the flag is recognized.
    let (_stdout, _stderr, status) =
        run_cli(&["extract", "http://example.com", "--max-chars", "100000"]);
    assert!(
        status.success(),
        "--max-chars 100000 should be a recognized flag/value: stderr={_stderr}"
    );
}

#[test]
fn test_describe_outputs_valid_json_with_expected_keys() {
    // `gthings describe --output json` must emit valid JSON wrapped in the
    // standard {status, data, error} envelope. The machine-parseable usage
    // guide an AI agent needs to self-discover the CLI — subcommands,
    // strategies, engines, operators, output_schema — lives under `data`.
    let (stdout, _stderr, status) = run_cli(&["describe", "--output", "json"]);
    assert!(
        status.success(),
        "describe --output json should succeed: stderr={}",
        _stderr
    );

    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!("describe --output json should emit valid JSON: {e}\nstdout={stdout}")
    });

    // Envelope shape first: {status, data, error, trace_id}.
    let envelope = parsed
        .as_object()
        .unwrap_or_else(|| panic!("describe output should be a JSON object: {stdout}"));
    for key in ["status", "data", "error", "trace_id"] {
        assert!(
            envelope.contains_key(key),
            "describe output should contain envelope key '{key}'"
        );
    }
    assert_eq!(
        parsed["status"], "ok",
        "describe should emit a success envelope"
    );
    assert!(
        parsed["error"].is_null(),
        "describe success envelope should have a null error"
    );
    assert!(
        parsed["trace_id"].as_str().is_some_and(|t| !t.is_empty()),
        "describe envelope should carry a non-empty trace_id"
    );

    // The guide itself now lives under `data`.
    let guide = parsed["data"]
        .as_object()
        .unwrap_or_else(|| panic!("describe envelope data should be an object: {stdout}"));
    for key in [
        "subcommands",
        "strategies",
        "engines",
        "operators",
        "output_schema",
    ] {
        assert!(
            guide.contains_key(key),
            "describe data should contain key '{key}'"
        );
    }

    // Spot-check the content of a few keys.
    assert!(
        parsed["data"]["strategies"]
            .as_object()
            .map(|s| s.contains_key("harvest"))
            .unwrap_or(false),
        "strategies should include 'harvest'"
    );
    assert!(
        parsed["data"]["engines"]
            .as_object()
            .map(|e| e.contains_key("google"))
            .unwrap_or(false),
        "engines should include 'google'"
    );
    assert!(
        parsed["data"]["operators"]
            .as_object()
            .map(|o| o.contains_key("site:"))
            .unwrap_or(false),
        "operators should include 'site:'"
    );
    assert!(
        parsed["data"]["output_schema"]
            .as_object()
            .map(|o| o.contains_key("status"))
            .unwrap_or(false),
        "output_schema should include 'status'"
    );
}

#[test]
fn test_describe_help_lists_subcommand() {
    // The describe subcommand must be discoverable via --help.
    let (stdout, _stderr, status) = run_cli(&["--help"]);
    assert!(status.success());
    assert!(
        stdout.contains("describe"),
        "--help should list the 'describe' subcommand"
    );
}

#[test]
fn test_config_emits_valid_envelope() {
    // `gthings config --output json` must emit the resolved env+defaults
    // configuration wrapped in the standard {status, data, error} envelope
    // (PROPOSAL §9) so Go can validate its boot-time assumptions at boot.
    let (stdout, _stderr, status) = run_cli(&["config", "--output", "json"]);
    assert!(
        status.success(),
        "config --output json should succeed: stderr={_stderr}"
    );

    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("config should emit valid JSON: {e}\nstdout={stdout}"));
    let envelope = parsed
        .as_object()
        .unwrap_or_else(|| panic!("config output should be a JSON object: {stdout}"));
    for key in ["status", "data", "error", "trace_id"] {
        assert!(
            envelope.contains_key(key),
            "config output should contain envelope key '{key}'"
        );
    }
    assert_eq!(
        parsed["status"], "ok",
        "config should emit a success envelope"
    );
    assert!(
        parsed["error"].is_null(),
        "config success envelope should have a null error"
    );
    assert!(
        parsed["trace_id"].as_str().is_some_and(|t| !t.is_empty()),
        "config envelope should carry a non-empty trace_id"
    );

    // The resolved config lives under `data`.
    let data = parsed["data"]
        .as_object()
        .unwrap_or_else(|| panic!("config envelope data should be an object: {stdout}"));
    for key in [
        "cdp_host",
        "user_agent",
        "update_disabled",
        "reputation_dir",
        "reputation_ttl_secs",
        "serve_bind",
        "command_timeouts",
    ] {
        assert!(
            data.contains_key(key),
            "config data should contain key '{key}'"
        );
    }
    assert_eq!(data["serve_bind"], "127.0.0.1:9080", "default serve bind");
}

#[test]
fn test_serve_and_config_help() {
    // Both `serve` and `config` must be discoverable via subcommand help.
    for sub in ["serve", "config"] {
        let (stdout, stderr, status) = run_cli(&[sub, "--help"]);
        assert!(
            status.success(),
            "{sub} --help should succeed: stderr={stderr}"
        );
        assert!(stdout.contains(sub), "{sub} --help should mention '{sub}'");
    }
}

#[test]
fn test_health_subcommand_exists() {
    // `gthings health` is the liveness probe: it must exit 0 when a CDP
    // browser is running and 1 otherwise. Either way it must emit the
    // standard {status, data, error, trace_id} envelope so downstream
    // orchestration (e.g. Go) has a single parse path.
    let (stdout, stderr, status) = run_cli(&["health", "--output", "json"]);
    assert!(
        status.code().is_some(),
        "health should be a recognized subcommand: stderr={stderr}"
    );
    assert!(
        status.code() == Some(0) || status.code() == Some(1),
        "health must exit 0 (browser running) or 1 (not running), got {:?}",
        status.code()
    );

    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("health should emit valid JSON: {e}\nstdout={stdout}"));
    let envelope = parsed
        .as_object()
        .unwrap_or_else(|| panic!("health output should be a JSON object: {stdout}"));
    for key in ["status", "data", "error", "trace_id"] {
        assert!(
            envelope.contains_key(key),
            "health output should contain envelope key '{key}'"
        );
    }
    assert!(
        parsed["status"] == "ok" || parsed["status"] == "error",
        "health envelope status should be 'ok' or 'error'"
    );
    assert!(
        parsed["trace_id"].as_str().is_some_and(|t| !t.is_empty()),
        "health envelope should carry a non-empty trace_id"
    );

    // Exit code and envelope status must agree.
    let running = status.code() == Some(0);
    assert_eq!(
        parsed["status"],
        if running { "ok" } else { "error" },
        "exit code and envelope status should agree"
    );
}
