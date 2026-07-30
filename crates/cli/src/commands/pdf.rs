//! PDF text extraction (`pdf-url` and `pdf-file` commands).

use crate::commands::{UniversalFlags, emit_output};
use gthings_common::pagination::ExtractParams;
use gthings_extraction::PdfExtractor;

/// Extract text from PDF at URL.
pub(crate) async fn cmd_pdf_url(
    flags: &UniversalFlags,
    url: &str,
    max_chars: usize,
    offset: usize,
) -> i32 {
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (compatible; gthings/0.5)")
        .timeout(std::time::Duration::from_secs(flags.timeout))
        .build()
        .expect("reqwest Client::builder() with default config should never fail");

    let resp = match client.get(url).send().await {
        Ok(r) => r,
        Err(e) => {
            emit_output(
                None,
                Some(("PDF_FETCH_FAILED", &e.to_string(), "Check URL")),
                flags.resolved_output(),
                flags.query.as_deref(),
            );
            return 1;
        }
    };

    if !resp.status().is_success() {
        emit_output(
            None,
            Some((
                "PDF_HTTP_ERROR",
                &format!("HTTP {}", resp.status()),
                "Verify URL",
            )),
            flags.resolved_output(),
            flags.query.as_deref(),
        );
        return 1;
    }

    let bytes = match resp.bytes().await {
        Ok(b) => b.to_vec(),
        Err(e) => {
            emit_output(
                None,
                Some(("PDF_READ_FAILED", &e.to_string(), "Retry")),
                flags.resolved_output(),
                flags.query.as_deref(),
            );
            return 1;
        }
    };

    let extractor = PdfExtractor;
    let params = ExtractParams { offset, max_chars };
    match extractor.extract_article(url, &bytes, &params) {
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
                Some(("PDF_EXTRACT_FAILED", &e.to_string(), "Try a different PDF")),
                flags.resolved_output(),
                flags.query.as_deref(),
            );
            1
        }
    }
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
    let extractor = PdfExtractor;
    let params = ExtractParams { offset, max_chars };
    match extractor.extract_article(&url, &bytes, &params) {
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
                Some((
                    "PDF_EXTRACT_FAILED",
                    &e.to_string(),
                    "File may not be a valid PDF",
                )),
                flags.resolved_output(),
                flags.query.as_deref(),
            );
            1
        }
    }
}
