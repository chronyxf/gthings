// Allow dead code — these helpers are used by ignored e2e tests
#![allow(dead_code)]

use std::path::PathBuf;
use std::process::Command;

/// Path to the gthings binary.
pub fn gthings_bin() -> PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_gthings") {
        return PathBuf::from(path);
    }
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let candidates = [
        base.join("target/debug/gthings"),
        base.join("target/release/gthings"),
    ];
    for c in &candidates {
        if c.exists() {
            return c.clone();
        }
    }
    panic!("gthings binary not found. Build first: cargo build -p gthings");
}

/// Create a gthings Command.
pub fn gthings() -> Command {
    Command::new(gthings_bin())
}

/// Create a minimal valid PDF for testing.
pub fn test_pdf_bytes() -> Vec<u8> {
    let mut pdf = Vec::new();
    pdf.extend_from_slice(b"%PDF-1.4\n%\xff\xff\xff\xff\n");
    pdf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    pdf.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");
    pdf.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>\nendobj\n");
    pdf.extend_from_slice(b"4 0 obj\n<< /Length 44 >>\nstream\nBT /F1 12 Tf 100 700 Td (Hello PDF World) Tj ET\nendstream\nendobj\n");
    pdf.extend_from_slice(
        b"5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n",
    );
    pdf.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n0000000010 00000 n \n0000000079 00000 n \n0000000159 00000 n \n0000000259 00000 n \n0000000395 00000 n \n");
    pdf.extend_from_slice(b"trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n510\n%%EOF\n");
    pdf
}

/// Write test PDF to temp dir, return path.
pub fn write_test_pdf() -> PathBuf {
    let dir = std::env::temp_dir().join("gthings-test-int");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("test.pdf");
    std::fs::write(&path, test_pdf_bytes()).unwrap();
    path
}

/// Assert output status is success.
pub fn assert_ok(output: &std::process::Output) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "Command failed (exit={})\nSTDERR: {}",
        output.status.code().unwrap_or(-1),
        stderr
    );
}

/// Assert stdout is valid JSON.
pub fn assert_json(output: &std::process::Output) -> serde_json::Value {
    assert_ok(output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).expect("Output should be valid JSON")
}

/// Parse JSON output from a CLI command.
/// Finds the first line that starts with `{` or `[` (skips ANSI-colored log lines).
pub fn parse_json(output: &std::process::Output) -> serde_json::Value {
    let stdout = String::from_utf8_lossy(&output.stdout);

    let json_start = stdout
        .lines()
        .position(|line| {
            let trimmed = line.trim();
            trimmed.starts_with('{') || trimmed.starts_with('[')
        });

    match json_start {
        Some(line_idx) => {
            let json_str: String = stdout
                .lines()
                .skip(line_idx)
                .collect::<Vec<_>>()
                .join("\n");
            serde_json::from_str(&json_str)
                .unwrap_or_else(|e| panic!("Invalid JSON: {}\nFirst 500 chars: {}", e, &json_str[..std::cmp::min(500, json_str.len())]))
        }
        None => panic!(
            "No JSON found in output:\nstdout: {}\nstderr: {}",
            stdout,
            String::from_utf8_lossy(&output.stderr)
        ),
    }
}

/// Run gthings with given args, return parsed JSON and raw output.
pub fn run_gthings(args: &[&str]) -> (serde_json::Value, std::process::Output) {
    let output = gthings()
        .args(args)
        .output()
        .expect("gthings execution failed");
    let json = parse_json(&output);
    (json, output)
}

use std::time::Duration;
use std::net::TcpStream;

/// Wait for port 9222 to become available (not in TIME_WAIT)
pub fn wait_for_port(timeout_secs: u64) -> bool {
    let start = std::time::Instant::now();
    // First try harder to kill whatever is on the port
    let _ = std::process::Command::new("pkill")
        .args(["-f", "remote-debugging-port=9222"])
        .output();
    let _ = std::process::Command::new("pkill")
        .args(["-f", "Dia.*9222"])
        .output();

    while start.elapsed().as_secs() < timeout_secs {
        match TcpStream::connect_timeout(
            &"127.0.0.1:9222".parse().unwrap(),
            Duration::from_millis(200),
        ) {
            Err(_) => return true, // Port is free
            Ok(_) => {
                // Port in use — try to kill the holder
                let _ = std::process::Command::new("pkill")
                    .args(["-f", "remote-debugging-port=9222"])
                    .output();
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

/// Kill any existing browser on port 9222
pub fn stop_existing_browser(bin: &std::path::Path) {
    let _ = std::process::Command::new(bin)
        .args(["browser", "stop"])
        .output();
    std::thread::sleep(Duration::from_millis(500));
}
