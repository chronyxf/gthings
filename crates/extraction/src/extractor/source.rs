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
            SourceType::GitHub
        } else if lower.ends_with(".pdf") || lower.contains("/pdf/") {
            SourceType::Pdf
        } else {
            SourceType::Web
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_source_type_arxiv() {
        assert_eq!(
            SourceType::from_url("https://arxiv.org/abs/2401.12345"),
            SourceType::Arxiv
        );
    }

    #[test]
    fn test_source_type_pdf() {
        assert_eq!(
            SourceType::from_url("https://example.com/paper.pdf"),
            SourceType::Pdf
        );
    }

    #[test]
    fn test_source_type_github() {
        assert_eq!(
            SourceType::from_url("https://github.com/user/repo"),
            SourceType::GitHub
        );
    }

    #[test]
    fn test_source_type_web() {
        assert_eq!(
            SourceType::from_url("https://example.com/article"),
            SourceType::Web
        );
    }
}
