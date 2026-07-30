use async_trait::async_trait;
use gthings_common::pagination::ExtractParams;
use gthings_common::provenance::{ExtractionMethod as ProvenanceMethod, Provenance};
use reqwest::Response;
use url::Url;

use crate::article::{Article, ExtractionError, ExtractionMethod};
use crate::extractor::{Extractor, SourceType};
use crate::pdf::PdfExtractor;
use crate::web::WebExtractor;

/// Check if the HTTP response indicates rate-limiting (429) and return
/// a `RateLimited` error with the parsed `Retry-After` header if so.
pub(crate) fn check_rate_limit(resp: &Response, detail: String) -> Result<(), ExtractionError> {
    if resp.status().as_u16() == 429 {
        let retry_after = match resp.headers().get("retry-after") {
            Some(v) => {
                let s = v
                    .to_str()
                    .map_err(|_| ExtractionError::Parse("non-UTF8 Retry-After header".into()))?;
                Some(s.parse::<u64>().map_err(|e| {
                    ExtractionError::Parse(format!("invalid Retry-After value: {e}"))
                })?)
            }
            None => None,
        };
        return Err(ExtractionError::RateLimited {
            detail,
            retry_after,
        });
    }
    Ok(())
}

/// Auto-dispatches extraction to the right extractor based on URL.
pub struct AutoExtractor {
    client: reqwest::Client,
    web: WebExtractor,
    pdf: PdfExtractor,
    max_content_bytes: u64,
}

impl AutoExtractor {
    /// Create a new AutoExtractor with a shared HTTP client reference.
    /// The client is cloned internally; callers can pass `http_client()` directly.
    pub fn new(client: &reqwest::Client) -> Self {
        let client = client.clone();
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
    async fn dispatch_extract(
        &self,
        url: &str,
        params: ExtractParams,
    ) -> Result<Article, ExtractionError> {
        let source_type = SourceType::from_url(url);

        match source_type {
            SourceType::Arxiv => self.extract_arxiv(url, params).await,
            SourceType::Pdf => self.extract_pdf(url, params).await,
            SourceType::GitHub => self.extract_github(url, params).await,
            SourceType::Web => self.web.extract(url.to_string(), params).await,
        }
    }

    /// Handle arXiv URLs: download the PDF for full extraction,
    /// then merge with abstract page metadata.
    async fn extract_arxiv(
        &self,
        url: &str,
        params: ExtractParams,
    ) -> Result<Article, ExtractionError> {
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
        let pdf_result = self.extract_pdf(&pdf_url, params).await;

        match pdf_result {
            Ok(mut article) => {
                // Mark as arXiv extraction
                article.extraction.method = ExtractionMethod::ArxivOai;
                article.extraction.confidence = crate::article::round_score(article.quality.score);

                // Try to get richer metadata from abstract page
                if article.source.author.is_none() || article.source.published.is_none() {
                    if let Ok(abs_article) =
                        self.web.extract(abs_url, ExtractParams::default()).await
                    {
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
                let mut article = self.web.extract(abs_url, ExtractParams::default()).await?;
                article.extraction.method = ExtractionMethod::ArxivOai;
                article.extraction.confidence = crate::article::round_score(article.quality.score);
                Ok(article)
            }
        }
    }

    /// Handle PDF URLs: download bytes, then extract.
    async fn extract_pdf(
        &self,
        url: &str,
        params: ExtractParams,
    ) -> Result<Article, ExtractionError> {
        let resp = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| ExtractionError::Http(format!("pdf fetch: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            check_rate_limit(&resp, format!("Rate limited while fetching PDF {url}"))?;
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

        self.pdf.extract((url.to_string(), bytes), params).await
    }

    /// Handle GitHub URLs with intelligent routing:
    ///
    /// | Pattern                | Action                                          |
    /// |------------------------|-------------------------------------------------|
    /// | `/blob/` or `/tree/`   | Rewrite to `raw.githubusercontent.com`           |
    /// | `.diff` / `.patch`     | Direct HTTP fetch (follows to `patch-diff...`)   |
    /// | `/owner/repo` (root)   | Fetch `raw.githubusercontent.com/.../README.md`  |
    /// | Everything else        | Fall through to `WebExtractor` (Readability)     |
    async fn extract_github(
        &self,
        url: &str,
        params: ExtractParams,
    ) -> Result<Article, ExtractionError> {
        let parsed = Url::parse(url)
            .map_err(|e| ExtractionError::Http(format!("invalid github url: {e}")))?;
        let path = parsed.path();

        // Determine which raw URL to fetch, or None (fall through to web extraction)
        let maybe_raw: Option<String> = {
            if path.contains("/blob/") {
                Some(
                    url.replace("github.com", "raw.githubusercontent.com")
                        .replace("/blob/", "/"),
                )
            } else if path.contains("/tree/") {
                Some(
                    url.replace("github.com", "raw.githubusercontent.com")
                        .replace("/tree/", "/"),
                )
            } else if path.ends_with(".diff") || path.ends_with(".patch") {
                // Fetch directly from github.com; reqwest follows the redirect
                // to patch-diff.githubusercontent.com automatically.
                Some(url.to_string())
            } else {
                // Repo root: path has exactly two non-empty segments (owner/repo)
                let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
                if segments.len() == 2 {
                    Some(format!(
                        "https://raw.githubusercontent.com/{}/{}/master/README.md",
                        segments[0], segments[1]
                    ))
                } else {
                    None
                }
            }
        };

        match maybe_raw {
            Some(raw_url) => self.fetch_raw_github(&raw_url, url, params).await,
            None => self.web.extract(url.to_string(), params).await,
        }
    }

    /// Fetch a raw GitHub file and wrap it in an [`Article`] with [`ContentTree::Code`].
    async fn fetch_raw_github(
        &self,
        raw_url: &str,
        original_url: &str,
        params: ExtractParams,
    ) -> Result<Article, ExtractionError> {
        let resp = self
            .client
            .get(raw_url)
            .send()
            .await
            .map_err(|e| ExtractionError::Http(format!("github raw fetch: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            check_rate_limit(&resp, format!("Rate limited while fetching {raw_url}"))?;
            return Err(ExtractionError::Http(format!("GitHub HTTP {status}")));
        }

        let content = resp
            .text()
            .await
            .map_err(|e| ExtractionError::Http(format!("github read: {e}")))?;

        let language = Self::detect_language(original_url);
        let total_len = content.len();
        let line_count = content.lines().count();

        // Apply offset and max_chars slicing
        let effective_content: String = content
            .chars()
            .skip(params.offset)
            .take(params.max_chars)
            .collect();
        let effective_len = effective_content.len();

        let pagination = gthings_common::pagination::build_pagination(
            &params,
            original_url,
            total_len,
            effective_len,
        )
        .map_err(|e| ExtractionError::Parse(e.to_string()))?;

        let now = chrono::Utc::now();
        let provenance = Provenance {
            source_url: original_url.to_string(),
            method: ProvenanceMethod::Github,
            agent: gthings_common::GTHINGS_AGENT.into(),
            accessed_at: now,
            duration_ms: 0,
            derived_from: None,
        };

        Ok(Article {
            url: original_url.to_string(),
            title: String::new(),
            source: crate::article::SourceInfo {
                author: None,
                published: None,
                site_name: "GitHub".into(),
                domain_authority: crate::extractor::compute_domain_authority(original_url),
                language: None,
            },
            extraction: crate::article::ExtractionInfo {
                method: ExtractionMethod::RawFile,
                confidence: crate::article::round_score(1.0),
                accessed_at: now.to_rfc3339(),
                duration_ms: 0,
            },
            body: crate::article::ContentTree::Code {
                language,
                content: effective_content,
                file_path: original_url.to_string(),
                line_count,
            },
            signals: crate::article::ContinuationSignals {
                truncated: pagination.truncated,
                total_length: total_len,
                returned_length: effective_len,
                is_paywall: false,
                is_bot_blocked: false,
                is_empty_shell: total_len < 50,
                related_urls: Vec::new(),
            },
            quality: crate::article::QualityScore {
                score: crate::article::round_score(0.9),
                is_ok: true,
                reasons: vec![],
                entropy_bits_per_char: 0.0,
            },
            provenance: Some(provenance),
            pagination: Some(pagination),
        })
    }

    /// Detect programming language from file extension.
    fn detect_language(url: &str) -> String {
        let path = url.split('?').next().unwrap();
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

#[async_trait]
impl Extractor for AutoExtractor {
    type Input = String;

    async fn extract(
        &self,
        input: String,
        params: ExtractParams,
    ) -> Result<Article, ExtractionError> {
        self.dispatch_extract(&input, params).await
    }

    fn method(&self) -> ExtractionMethod {
        ExtractionMethod::Readability
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

    #[test]
    fn test_detect_language_case_insensitive() {
        // Extension matching is case-sensitive; recognized lowercase extensions work
        assert_eq!(AutoExtractor::detect_language("file.rs"), "rust");
        assert_eq!(AutoExtractor::detect_language("file.py"), "python");
        assert_eq!(AutoExtractor::detect_language("file.md"), "markdown");
    }

    #[test]
    fn test_detect_language_no_extension() {
        // Files without an extension return the filename as the "language"
        assert_eq!(AutoExtractor::detect_language("file"), "file");
        assert_eq!(AutoExtractor::detect_language(""), "");
    }

    #[test]
    fn test_detect_language_with_query() {
        // URL with query params should still detect language
        assert_eq!(
            AutoExtractor::detect_language("script.js?version=2"),
            "javascript"
        );
    }
}
