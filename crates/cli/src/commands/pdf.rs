//! PDF text extraction (`pdf-url` and `pdf-file` commands).

use crate::commands::{UniversalFlags, emit_output};
use gthings_common::pagination::ExtractParams;
use gthings_extraction::PdfExtractor;

/// Emit a uniform error envelope for PDF extraction failures.
fn emit_error(code: &str, detail: &str, hint: &str, flags: &UniversalFlags) {
    emit_output(
        None,
        Some((code, detail, hint)),
        flags.resolved_output(),
        flags.query.as_deref(),
    );
}

/// Shared helper: run `PdfExtractor::extract_article` and emit the result.
fn handle_pdf_extraction(
    flags: &UniversalFlags,
    url: &str,
    bytes: &[u8],
    params: ExtractParams,
    hint: &str,
) -> i32 {
    let extractor = PdfExtractor;
    match extractor.extract_article(url, bytes, &params) {
        Ok(article) => {
            let value = serde_json::json!(article);
            emit_output(
                Some(value),
                None,
                flags.resolved_output(),
                flags.query.as_deref(),
            );
            0
        }
        Err(e) => {
            emit_output(
                None,
                Some(("PDF_EXTRACT_FAILED", &e.to_string(), hint)),
                flags.resolved_output(),
                flags.query.as_deref(),
            );
            1
        }
    }
}

/// Extract text from PDF at URL.
pub(crate) async fn cmd_pdf_url(
    flags: &UniversalFlags,
    url: &str,
    max_chars: usize,
    offset: usize,
) -> i32 {
    let client = crate::commands::http_client();

    let resp = match client.get(url).send().await {
        Ok(r) => r,
        Err(e) => {
            emit_error("PDF_FETCH_FAILED", &e.to_string(), "Check URL", flags);
            return 1;
        }
    };

    if !resp.status().is_success() {
        emit_error(
            "PDF_HTTP_ERROR",
            &format!("HTTP {}", resp.status()),
            "Verify URL",
            flags,
        );
        return 1;
    }

    let bytes = match resp.bytes().await {
        Ok(b) => b,
        Err(e) => {
            emit_error("PDF_READ_FAILED", &e.to_string(), "Retry", flags);
            return 1;
        }
    };

    let params = ExtractParams { offset, max_chars };
    handle_pdf_extraction(flags, url, &bytes, params, "Try a different PDF")
}

/// Extract text from local PDF file.
pub(crate) async fn cmd_pdf_file(
    flags: &UniversalFlags,
    path: &std::path::Path,
    max_chars: usize,
    offset: usize,
) -> i32 {
    let bytes = match tokio::fs::read(path).await {
        Ok(b) => b,
        Err(e) => {
            emit_output(
                None,
                Some(("PDF_READ_FAILED", &e.to_string(), "Check file path")),
                flags.resolved_output(),
                flags.query.as_deref(),
            );
            return 1;
        }
    };

    let url = format!("file://{}", path.display());
    let params = ExtractParams { offset, max_chars };
    handle_pdf_extraction(flags, &url, &bytes, params, "File may not be a valid PDF")
}
