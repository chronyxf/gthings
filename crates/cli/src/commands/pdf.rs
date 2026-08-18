//! PDF text extraction (`pdf-url` and `pdf-file` commands).

use crate::util::{UniversalFlags, emit_error, emit_success};
use gthings_common::pagination::ExtractParams;
use gthings_common::taxonomy::ErrorCode;
use gthings_extraction::PdfExtractor;

/// `file://` URL prefix for local PDF paths.
const FILE_URL_PREFIX: &str = "file://";

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
            emit_success(flags, serde_json::json!(article));
            0
        }
        Err(e) => {
            emit_error(flags, ErrorCode::ExtractFailed, &e.to_string(), hint);
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
    let client = crate::util::http_client();

    let resp = match client.get(url).send().await {
        Ok(r) => r,
        Err(e) => {
            emit_error(flags, ErrorCode::ExtractFailed, &e.to_string(), "Check URL");
            return 1;
        }
    };

    if !resp.status().is_success() {
        emit_error(
            flags,
            ErrorCode::ExtractFailed,
            &format!("HTTP {}", resp.status()),
            "Verify URL",
        );
        return 1;
    }

    let bytes = match resp.bytes().await {
        Ok(b) => b,
        Err(e) => {
            emit_error(flags, ErrorCode::ExtractFailed, &e.to_string(), "Retry");
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
            emit_error(
                flags,
                ErrorCode::ExtractFailed,
                &e.to_string(),
                "Check file path",
            );
            return 1;
        }
    };

    let url = format!("{FILE_URL_PREFIX}{}", path.display());
    let params = ExtractParams { offset, max_chars };
    handle_pdf_extraction(flags, &url, &bytes, params, "File may not be a valid PDF")
}
