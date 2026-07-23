use std::process::Command;

#[test]
fn test_binary_exists() {
    let output = Command::new("cargo")
        .args(["run", "--", "--help"])
        .output()
        .expect("failed to execute binary");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("search") || stdout.contains("follow"));
}
