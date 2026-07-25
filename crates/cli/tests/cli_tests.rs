use std::process::Command;

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
        stdout.contains("follow"),
        "--help should list 'follow' subcommand"
    );
    assert!(
        stdout.contains("batch"),
        "--help should list 'batch' subcommand"
    );
    assert!(
        stdout.contains("status"),
        "--help should list 'status' subcommand"
    );
}
