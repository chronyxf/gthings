use gthings_common::pagination::ExtractParams;

use crate::article::{Article, ExtractionError, ExtractionMethod};

/// Every extractor implements this trait.
/// Input varies by source, output is always Article.
/// `params` controls offset/max_chars slicing of the extracted text.
#[async_trait::async_trait]
pub trait Extractor: Send + Sync {
    type Input: Send + 'static;
    async fn extract(
        &self,
        input: Self::Input,
        params: ExtractParams,
    ) -> Result<Article, ExtractionError>;
    fn method(&self) -> ExtractionMethod;
}

/// Source type detected from URL.
#[derive(Debug, Clone, PartialEq)]
pub enum SourceType {
    Web,
    Pdf,
    Arxiv,
    GitHub,
}

impl SourceType {
    /// Detect source type from URL string.
    pub fn from_url(url: &str) -> Self {
        let lower = url.to_lowercase();
        if lower.contains("arxiv.org") {
            SourceType::Arxiv
        } else if lower.contains("github.com") {
            // Check if it's a raw file URL or a repo URL
            if lower.contains("raw.githubusercontent.com") || lower.contains("github.com") {
                SourceType::GitHub
            } else {
                SourceType::Web
            }
        } else if lower.ends_with(".pdf") || lower.contains("/pdf/") {
            SourceType::Pdf
        } else {
            SourceType::Web
        }
    }
}

/// Compute a domain authority score (0.0-1.0) for a hostname.
///
/// Uses a curated list of academic, technical, news, and government domains.
/// Unknown domains default to 0.5. The score helps AI agents assess source trustworthiness.
pub fn domain_authority(host: &str) -> f32 {
    authority_for_domain(host) as f32
}

/// Internal helper: look up authority value for a normalized domain string.
fn authority_for_domain(domain: &str) -> f64 {
    let domain = domain.to_lowercase();

    // High-authority academic/scholarly domains
    let high: [&str; 14] = [
        "arxiv.org",
        "scholar.google.com",
        "pubmed.ncbi.nlm.nih.gov",
        "doi.org",
        "ieeexplore.ieee.org",
        "dl.acm.org",
        "springer.com",
        "elsevier.com",
        "sciencedirect.com",
        "nature.com",
        "science.org",
        "plos.org",
        "wikipedia.org",
        "wikidata.org",
    ];

    // Medium-high authority (technical, educational, government)
    let med_high: [&str; 28] = [
        "github.com",
        "gitlab.com",
        "bitbucket.org",
        "docs.rs",
        "crates.io",
        "pypi.org",
        "npmjs.com",
        "stackoverflow.com",
        "stackexchange.com",
        "rust-lang.org",
        "mozilla.org",
        "chromium.org",
        "openai.com",
        "deepmind.com",
        "research.google.com",
        "mit.edu",
        "stanford.edu",
        "ox.ac.uk",
        "cam.ac.uk",
        "ibm.com",
        "microsoft.com",
        "google.com",
        "apple.com",
        "oracle.com",
        "redhat.com",
        "nginx.com",
        "docker.com",
        "cloudflare.com",
    ];

    // News outlets
    let news: [&str; 13] = [
        "reuters.com",
        "ap.org",
        "bbc.com",
        "bbc.co.uk",
        "nytimes.com",
        "wsj.com",
        "economist.com",
        "theguardian.com",
        "washingtonpost.com",
        "bloomberg.com",
        "ft.com",
        "npr.org",
        "techcrunch.com",
    ];

    if high
        .iter()
        .any(|h| domain == *h || domain.ends_with(&format!(".{}", h)))
    {
        0.9
    } else if med_high
        .iter()
        .any(|h| domain == *h || domain.ends_with(&format!(".{}", h)))
    {
        0.8
    } else if news
        .iter()
        .any(|h| domain == *h || domain.ends_with(&format!(".{}", h)))
    {
        0.7
    } else if domain.ends_with(".edu") || domain.ends_with(".gov") || domain.ends_with(".ac.uk") {
        0.8
    } else {
        0.5
    }
}

/// Compute a domain authority score (0.0-1.0) based on recognized domains.
///
/// Uses a curated list of academic, technical, news, and government domains.
/// Unknown domains default to 0.5. The score helps AI agents assess source trustworthiness.
pub(crate) fn compute_domain_authority(url: &str) -> f64 {
    let domain = url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or("")
        .to_lowercase();
    authority_for_domain(&domain)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_domain_authority_high() {
        assert!(domain_authority("arxiv.org") > 0.8);
        assert!(domain_authority("www.arxiv.org") > 0.8);
    }

    #[test]
    fn test_domain_authority_medium() {
        assert!(domain_authority("github.com") >= 0.5);
        assert!(domain_authority("stackoverflow.com") >= 0.5);
    }

    #[test]
    fn test_domain_authority_unknown() {
        let auth = domain_authority("example-unknown-site.com");
        assert!(auth >= 0.0 && auth <= 0.5);
    }

    #[test]
    fn test_domain_authority_empty() {
        let auth = domain_authority("");
        assert!(auth >= 0.0);
    }
}
