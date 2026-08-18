//! Programming-language detection from file extensions.

use super::AutoExtractor;

impl AutoExtractor {
    /// Detect programming language from file extension.
    pub(super) fn detect_language(url: &str) -> String {
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
    fn test_detect_language_case_sensitive() {
        // Extension matching is case-sensitive: only lowercase extensions are
        // recognized; an uppercase extension falls through to the raw extension.
        assert_eq!(AutoExtractor::detect_language("file.rs"), "rust");
        assert_eq!(AutoExtractor::detect_language("file.py"), "python");
        assert_eq!(AutoExtractor::detect_language("file.md"), "markdown");
        assert_eq!(AutoExtractor::detect_language("file.RS"), "RS");
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
