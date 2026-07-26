//! `gthings extract` and `gthings pdf` — content extraction from URLs, PDFs.

use crate::commands::print_error;

/// Extract content from any URL using auto-detection.
pub(crate) async fn cmd_extract(url: &str, _max_chars: usize, json: bool) -> i32 {
    use gthings_extraction::dispatch::AutoExtractor;

    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (compatible; gthings/0.5)")
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("valid client config");

    let extractor = AutoExtractor::new(client);
    match extractor.extract(url).await {
        Ok(article) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&article).unwrap_or_default()
                );
            } else {
                use gthings_extraction::article::format_as_markdown;
                println!("{}", format_as_markdown(&article));
            }
            0
        }
        Err(e) => {
            print_error(
                "EXTRACT_FAILED",
                &e.to_string(),
                "Check URL and connectivity",
            );
            1
        }
    }
}

/// Extract text from PDF at URL.
pub(crate) async fn cmd_pdf_url(url: &str, json: bool) -> i32 {
    use gthings_extraction::PdfExtractor;

    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (compatible; gthings/0.5)")
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("valid client config");

    let resp = match client.get(url).send().await {
        Ok(r) => r,
        Err(e) => {
            print_error("PDF_FETCH_FAILED", &e.to_string(), "Check URL");
            return 1;
        }
    };

    if !resp.status().is_success() {
        print_error(
            "PDF_HTTP_ERROR",
            &format!("HTTP {}", resp.status()),
            "Verify URL",
        );
        return 1;
    }

    let bytes = match resp.bytes().await {
        Ok(b) => b.to_vec(),
        Err(e) => {
            print_error("PDF_READ_FAILED", &e.to_string(), "Retry");
            return 1;
        }
    };

    let extractor = PdfExtractor;
    match extractor.extract_article(url, &bytes) {
        Ok(article) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&article).unwrap_or_default()
                );
            } else {
                use gthings_extraction::article::format_as_markdown;
                println!("{}", format_as_markdown(&article));
            }
            0
        }
        Err(e) => {
            print_error("PDF_EXTRACT_FAILED", &e.to_string(), "Try a different PDF");
            1
        }
    }
}

/// Extract text from local PDF file.
pub(crate) async fn cmd_pdf_file(path: &std::path::Path, json: bool) -> i32 {
    use gthings_extraction::PdfExtractor;

    let bytes = match tokio::fs::read(path).await {
        Ok(b) => b,
        Err(e) => {
            print_error("PDF_READ_FAILED", &e.to_string(), "Check file path");
            return 1;
        }
    };

    let url = format!("file://{}", path.display());
    let extractor = PdfExtractor;
    match extractor.extract_article(&url, &bytes) {
        Ok(article) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&article).unwrap_or_default()
                );
            } else {
                use gthings_extraction::article::format_as_markdown;
                println!("{}", format_as_markdown(&article));
            }
            0
        }
        Err(e) => {
            print_error(
                "PDF_EXTRACT_FAILED",
                &e.to_string(),
                "File may not be a valid PDF",
            );
            1
        }
    }
}
