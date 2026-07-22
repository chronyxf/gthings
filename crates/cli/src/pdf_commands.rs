use std::path::Path;

use common::config::GthingsConfig;

/// Shared output helper for PDF extraction results.
fn output_pdf_result(text: &str, source: &str, pages: Option<usize>, json: bool) {
    if json {
        let value = serde_json::json!({
            "source": source,
            "text": text,
            "length": text.len(),
            "pages": pages,
        });
        println!("{}", serde_json::to_string_pretty(&value).unwrap());
    } else {
        println!("PDF: {}", source);
        if let Some(p) = pages {
            println!("Pages: {}", p);
        }
        println!("Length: {} chars", text.len());
        println!();
        // Show first 10 lines as preview
        for line in text.lines().take(10) {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                println!("  │ {}", trimmed);
            }
        }
        if text.lines().count() > 10 {
            println!("  │ … ({} more lines)", text.lines().count() - 10);
        }
    }
}

/// Handler for `pdf url <url>`
pub async fn handle_pdf_url(
    config: &GthingsConfig,
    url: &str,
    json: bool,
) -> Result<(), anyhow::Error> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(config.request_timeout_ms))
        .build()?;

    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to fetch PDF URL '{}': {}", url, e))?;

    let status = response.status();
    if !status.is_success() {
        return Err(anyhow::anyhow!(
            "PDF URL '{}' returned HTTP {} {}. Verify the URL is correct and accessible.",
            url,
            status.as_u16(),
            status.canonical_reason().unwrap_or("Unknown"),
        ));
    }

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .to_string();

    let bytes = response
        .bytes()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to read PDF content from '{}': {}", url, e))?;

    // Quick check: PDF files start with %PDF
    if bytes.len() < 4 || &bytes[..4] != b"%PDF" {
        return Err(anyhow::anyhow!(
            "URL '{}' returned content type '{}' ({} bytes) but does not appear to be a PDF. \
             The URL may point to an HTML page instead of a PDF file. \
             For arXiv papers, use the abstract page URL (https://arxiv.org/abs/XXXX.XXXXX) \
             and the PDF URL will be extracted automatically.",
            url,
            content_type,
            bytes.len(),
        ));
    }

    let text = extraction::PdfExtractor::extract(&bytes)
        .map_err(|e| anyhow::anyhow!("Failed to extract text from PDF at '{}': {}", url, e))?;
    let pages = extraction::PdfExtractor::count_pages(&bytes).ok();

    output_pdf_result(&text, url, pages, json);
    Ok(())
}

/// Handler for `pdf file <path>`
pub async fn handle_pdf_file(
    _config: &GthingsConfig,
    path: &Path,
    json: bool,
) -> Result<(), anyhow::Error> {
    let bytes = std::fs::read(path)?;

    let text = extraction::PdfExtractor::extract(&bytes)?;
    let pages = extraction::PdfExtractor::count_pages(&bytes).ok();

    let source = path.to_string_lossy();
    output_pdf_result(&text, &source, pages, json);
    Ok(())
}
