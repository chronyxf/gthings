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

/// Check if browser daemon tests should run.
pub fn daemon_available() -> bool {
    std::env::var("GTHINGS_TEST_DAEMON").is_ok()
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
