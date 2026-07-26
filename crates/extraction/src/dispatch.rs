use crate::article::{Article, ExtractionError, ExtractionMethod};
use crate::extractor::{Extractor, SourceType};
use crate::pdf::PdfExtractor;
use crate::web::WebExtractor;

/// Auto-dispatches extraction to the right extractor based on URL.
pub struct AutoExtractor {
    client: reqwest::Client,
    web: WebExtractor,
    pdf: PdfExtractor,
    max_content_bytes: u64,
}

impl AutoExtractor {
    /// Create a new AutoExtractor with shared HTTP client.
    pub fn new(client: reqwest::Client) -> Self {
        let web = WebExtractor::new(client.clone());
        Self {
            client,
            web,
            pdf: PdfExtractor,
            max_content_bytes: 50 * 1024 * 1024,
        }
    }

    /// Set max content size for PDF downloads.
    pub fn with_max_content(mut self, bytes: u64) -> Self {
        self.max_content_bytes = bytes;
        self
    }

    /// Extract content from a URL, auto-detecting the source type.
    pub async fn extract(&self, url: &str) -> Result<Article, ExtractionError> {
        let source_type = SourceType::from_url(url);

        match source_type {
            SourceType::Arxiv => self.extract_arxiv(url).await,
            SourceType::Pdf => self.extract_pdf(url).await,
            SourceType::GitHub => self.extract_github(url).await,
            SourceType::Web => self.web.extract(url.to_string()).await,
        }
    }

    /// Handle arXiv URLs: download the PDF for full extraction,
    /// then merge with abstract page metadata.
    async fn extract_arxiv(&self, url: &str) -> Result<Article, ExtractionError> {
        // Normalize URL: ensure we have a /pdf/ URL for downloading
        let pdf_url = if url.contains("/abs/") {
            url.replace("/abs/", "/pdf/")
        } else if !url.contains("/pdf/") {
            format!("{}/pdf", url.trim_end_matches('/'))
        } else {
            url.to_string()
        };

        // Also get the abstract URL for metadata
        let abs_url = pdf_url
            .replace("/pdf/", "/abs/")
            .trim_end_matches(".pdf")
            .to_string();

        // Try to download and extract the PDF
        let pdf_result = self.extract_pdf(&pdf_url).await;

        match pdf_result {
            Ok(mut article) => {
                // Mark as arXiv extraction
                article.extraction.method = ExtractionMethod::ArxivOai;
                article.extraction.confidence = crate::article::round_score(article.quality.score);

                // Try to get richer metadata from abstract page
                if article.source.author.is_none() || article.source.published.is_none() {
                    if let Ok(abs_article) = self.web.extract(abs_url).await {
                        if article.source.author.is_none() {
                            article.source.author = abs_article.source.author;
                        }
                        if article.source.published.is_none() {
                            article.source.published = abs_article.source.published;
                        }
                        if article.source.site_name.is_empty() {
                            article.source.site_name = abs_article.source.site_name;
                        }
                        if article.title.is_empty() {
                            article.title = abs_article.title;
                        }
                    }
                }

                Ok(article)
            }
            Err(_) => {
                // Fall back to abstract page HTML extraction
                let mut article = self.web.extract(abs_url).await?;
                article.extraction.method = ExtractionMethod::ArxivOai;
                article.extraction.confidence = crate::article::round_score(article.quality.score);
                Ok(article)
            }
        }
    }

    /// Handle PDF URLs: download bytes, then extract.
    async fn extract_pdf(&self, url: &str) -> Result<Article, ExtractionError> {
        let resp = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| ExtractionError::Http(format!("pdf fetch: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            return Err(ExtractionError::Http(format!("HTTP {status} for PDF")));
        }

        if let Some(content_length) = resp.content_length() {
            if content_length > self.max_content_bytes {
                return Err(ExtractionError::Http(format!(
                    "PDF too large: {content_length} bytes (max {})",
                    self.max_content_bytes
                )));
            }
        }

        let bytes = resp
            .bytes()
            .await
            .map_err(|e| ExtractionError::Http(format!("pdf read: {e}")))?
            .to_vec();

        self.pdf.extract((url.to_string(), bytes)).await
    }

    /// Handle GitHub URLs: rewrite to raw content URL.
    async fn extract_github(&self, url: &str) -> Result<Article, ExtractionError> {
        let raw_url = url
            .replace("github.com", "raw.githubusercontent.com")
            .replace("/blob/", "/");

        let resp = self
            .client
            .get(&raw_url)
            .send()
            .await
            .map_err(|e| ExtractionError::Http(format!("github raw fetch: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            return Err(ExtractionError::Http(format!("GitHub HTTP {status}")));
        }

        let content = resp
            .text()
            .await
            .map_err(|e| ExtractionError::Http(format!("github read: {e}")))?;

        let language = Self::detect_language(url);
        let total_length = content.len();
        let line_count = content.lines().count();

        Ok(Article {
            url: url.to_string(),
            title: String::new(),
            source: crate::article::SourceInfo {
                author: None,
                published: None,
                site_name: "GitHub".into(),
                domain_authority: crate::extractor::compute_domain_authority(url),
                language: None,
            },
            extraction: crate::article::ExtractionInfo {
                method: ExtractionMethod::RawFile,
                confidence: crate::article::round_score(1.0),
                accessed_at: chrono::Utc::now().to_rfc3339(),
                duration_ms: 0,
            },
            body: crate::article::ContentTree::Code {
                language,
                content,
                file_path: url.to_string(),
                line_count,
            },
            signals: crate::article::ContinuationSignals {
                truncated: false,
                total_length,
                returned_length: total_length,
                is_paywall: false,
                is_bot_blocked: false,
                is_empty_shell: total_length < 50,
                related_urls: Vec::new(),
            },
            quality: crate::article::QualityScore {
                score: crate::article::round_score(0.9),
                is_ok: true,
                reasons: vec![],
            },
        })
    }

    /// Detect programming language from file extension.
    fn detect_language(url: &str) -> String {
        let path = url.split('?').next().unwrap_or(url);
        if let Some(ext) = path.rsplit('.').next() {
            match ext {
                "rs" => "rust".into(),
                "py" => "python".into(),
                "js" | "mjs" => "javascript".into(),
                "ts" | "tsx" => "typescript".into(),
                "go" => "go".into(),
                "java" => "java".into(),
                "c" | "h" => "c".into(),
                "cpp" | "hpp" | "cc" | "cxx" => "cpp".into(),
                "md" | "mdx" => "markdown".into(),
                "json" => "json".into(),
                "yaml" | "yml" => "yaml".into(),
                "toml" => "toml".into(),
                "sh" | "bash" => "shell".into(),
                "css" => "css".into(),
                "html" | "htm" => "html".into(),
                "sql" => "sql".into(),
                "rb" => "ruby".into(),
                "swift" => "swift".into(),
                "kt" | "kts" => "kotlin".into(),
                _ => ext.to_string(),
            }
        } else {
            "unknown".into()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_language_rust() {
        assert_eq!(
            AutoExtractor::detect_language("https://github.com/user/repo/blob/main/src/lib.rs"),
            "rust"
        );
    }

    #[test]
    fn test_detect_language_python() {
        assert_eq!(
            AutoExtractor::detect_language("https://github.com/user/repo/blob/main/main.py"),
            "python"
        );
    }

    #[test]
    fn test_detect_language_markdown() {
        assert_eq!(
            AutoExtractor::detect_language("https://github.com/user/repo/blob/main/README.md"),
            "markdown"
        );
    }

    #[test]
    fn test_detect_language_unknown() {
        assert_eq!(
            AutoExtractor::detect_language("https://example.com/file.xyz"),
            "xyz"
        );
    }

    #[test]
    fn test_detect_language_javascript() {
        assert_eq!(
            AutoExtractor::detect_language("https://github.com/user/repo/blob/main/app.js"),
            "javascript"
        );
    }
}
