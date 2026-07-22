/// HTML content extraction utilities.
///
/// Ported from `skills/cdp/scripts/templates.ts` (extractionCode).
/// Provides CSS-selector-based content extraction, heading-based section
/// detection, and HTML tag stripping.
use regex::Regex;
use std::sync::OnceLock;

use common::GthingsError;

/// Extracted content from an HTML page.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExtractedContent {
    /// The extracted text content.
    pub content: String,
    /// Total length of the full text on the page.
    pub total_length: usize,
    /// Character offset into the full text where this content starts.
    pub offset: usize,
    /// Whether the content was truncated (always false — full extraction).
    pub truncated: bool,
    /// Detected sections (heading + content pairs).
    pub sections: Vec<Section>,
}

/// A content section identified by its heading.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Section {
    /// The heading text.
    pub heading: String,
    /// The content following this heading.
    pub content: String,
}

/// HTML content extractor.
///
/// Provides CSS-selector-based extraction, heading section detection,
/// and HTML tag stripping.
///
/// # Examples
///
/// ```ignore
/// let html = std::fs::read_to_string("page.html").unwrap();
/// let extracted = HtmlExtractor::extract(&html, "article, main, [role=main]").unwrap();
/// println!("Extracted {} characters", extracted.total_length);
/// ```
pub struct HtmlExtractor;

impl HtmlExtractor {
    /// Extract article content from HTML using a CSS selector.
    ///
    /// Finds the first element matching `selector`, extracts all text content
    /// from it, and detects section boundaries from heading tags (h1, h2, h3).
    ///
    /// If the selector does not match any element, falls back to `body`.
    ///
    /// # Errors
    ///
    /// Returns an error if the CSS selector is invalid.
    pub fn extract(html: &str, selector: &str) -> Result<ExtractedContent, GthingsError> {
        let doc = scraper::Html::parse_document(html);

        let sel = scraper::Selector::parse(selector)
            .map_err(|e| GthingsError::Parse(format!("Invalid CSS selector '{selector}': {e}")))?;

        let root = doc.select(&sel).next().unwrap_or_else(|| {
            // Fall back to body
            let body_sel = scraper::Selector::parse("body").unwrap();
            doc.select(&body_sel).next().unwrap_or(doc.root_element())
        });

        // Extract full text from the root element
        let full_text: String = root.text().collect::<Vec<_>>().join(" ");
        let total_length = full_text.len();

        // Detect sections from heading elements within the root
        let sections = Self::detect_sections_html(&root, &full_text);

        Ok(ExtractedContent {
            content: full_text.clone(),
            total_length,
            offset: 0,
            truncated: false,
            sections,
        })
    }

    /// Detect section boundaries from heading tags (h1, h2, h3).
    ///
    /// Given a plain text body, splits it into sections by looking for
    /// lines that match heading-like patterns (ALL CAPS, short lines
    /// ending with colon, or numbered sections).
    ///
    /// This is a heuristic for text that has already had HTML stripped.
    pub fn detect_sections(text: &str) -> Vec<Section> {
        if text.is_empty() {
            return Vec::new();
        }

        let mut sections: Vec<Section> = Vec::new();
        let mut current_heading = String::new();
        let mut current_content = String::new();

        for line in text.lines() {
            let trimmed = line.trim().to_string();
            if trimmed.is_empty() {
                continue;
            }

            if is_heading_line(&trimmed) {
                if !current_heading.is_empty() {
                    sections.push(Section {
                        heading: current_heading,
                        content: current_content.trim().to_string(),
                    });
                }
                current_heading = trimmed;
                current_content = String::new();
            } else if !current_heading.is_empty() {
                if !current_content.is_empty() {
                    current_content.push(' ');
                }
                current_content.push_str(&trimmed);
            }
        }

        // Push the last section
        if !current_heading.is_empty() {
            sections.push(Section {
                heading: current_heading,
                content: current_content.trim().to_string(),
            });
        }

        // If no headings were found but there's content, create a single section
        if sections.is_empty() && !text.trim().is_empty() {
            sections.push(Section {
                heading: String::new(),
                content: text.trim().to_string(),
            });
        }

        sections
    }

    /// Strip HTML tags, keeping text content.
    ///
    /// Removes all HTML tags and decodes common HTML entities.
    /// Uses regex-based tag removal for simplicity.
    ///
    /// # Examples
    ///
    /// ```
    /// # use extraction::HtmlExtractor;
    /// let text = HtmlExtractor::strip_tags("<p>Hello <b>world</b></p>");
    /// assert_eq!(text, "Hello world");
    /// ```
    pub fn strip_tags(html: &str) -> String {
        static TAG_RE: OnceLock<Regex> = OnceLock::new();
        let tag_re = TAG_RE.get_or_init(|| Regex::new(r"<[^>]+>").unwrap());
        let result = tag_re.replace_all(html, " ");

        static ENTITY_RE: OnceLock<Regex> = OnceLock::new();
        let entity_re = ENTITY_RE
            .get_or_init(|| Regex::new(r"&(amp|lt|gt|quot|nbsp|apos|#x?[0-9a-fA-F]+);").unwrap());
        let result = entity_re.replace_all(&result, |caps: &regex::Captures| -> String {
            match &caps[1] {
                "amp" => "&".to_string(),
                "lt" => "<".to_string(),
                "gt" => ">".to_string(),
                "quot" => "\"".to_string(),
                "nbsp" => " ".to_string(),
                "apos" => "'".to_string(),
                other if other.starts_with('#') => {
                    let code = if other.starts_with("#x") || other.starts_with("#X") {
                        u32::from_str_radix(&other[2..], 16)
                    } else {
                        u32::from_str_radix(&other[1..], 10)
                    };
                    code.ok()
                        .and_then(char::from_u32)
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| format!("&{other};"))
                }
                _ => format!("&{};", &caps[1]),
            }
        });

        // Collapse whitespace
        static WS_RE: OnceLock<Regex> = OnceLock::new();
        let ws_re = WS_RE.get_or_init(|| Regex::new(r"\s+").unwrap());
        let result = ws_re.replace_all(&result, " ");

        result.trim().to_string()
    }

    // ─── Private: section detection from parsed HTML ────────────────────

    /// Detect sections from a scraper HTML element tree.
    fn detect_sections_html(root: &scraper::ElementRef, full_text: &str) -> Vec<Section> {
        // Find all h1, h2, h3 elements within the root
        let heading_sel = match scraper::Selector::parse("h1, h2, h3") {
            Ok(sel) => sel,
            Err(_) => return Vec::new(),
        };

        let headings: Vec<(String, usize)> = root
            .select(&heading_sel)
            .filter_map(|el| {
                let text: String = el.text().collect();
                let trimmed = text.trim().to_string();
                if trimmed.len() > 1 {
                    let idx = full_text.find(&trimmed);
                    idx.map(|i| (trimmed, i))
                } else {
                    None
                }
            })
            .collect();

        if headings.is_empty() {
            return Vec::new();
        }

        let mut sections: Vec<Section> = Vec::new();
        for i in 0..headings.len() {
            let (heading, offset) = &headings[i];
            let content = if i + 1 < headings.len() {
                let next_offset = headings[i + 1].1;
                if *offset < next_offset {
                    full_text[*offset + heading.len()..next_offset]
                        .trim()
                        .to_string()
                } else {
                    String::new()
                }
            } else {
                full_text[*offset + heading.len()..].trim().to_string()
            };

            if !heading.is_empty() {
                sections.push(Section {
                    heading: heading.clone(),
                    content,
                });
            }
        }

        sections
    }
}

// ─── Private Helpers ─────────────────────────────────────────────────────

/// Check if a text line looks like a section heading.
fn is_heading_line(line: &str) -> bool {
    if line.len() < 3 {
        return false;
    }

    // Lines ending with colon (e.g., "Introduction:")
    if line.ends_with(':') {
        return true;
    }

    // ALL CAPS lines with at least 15 chars and mostly uppercase
    if line.len() >= 15 {
        let uppercase_or_space = line
            .chars()
            .filter(|c| c.is_ascii_uppercase() || c.is_whitespace())
            .count();
        if uppercase_or_space as f64 / line.len() as f64 > 0.7 {
            return true;
        }
    }

    // Numbered lines like "1. Title" or "1) Title"
    static NUM_RE: OnceLock<Regex> = OnceLock::new();
    let num_re = NUM_RE.get_or_init(|| Regex::new(r"^\d+[.)]\s").unwrap());
    if num_re.is_match(line) {
        return true;
    }

    // Short all-caps or title-case lines (at least 2 chars)
    if line.len() <= 60 && !line.ends_with('.') && !line.ends_with('!') && !line.ends_with('?') {
        // Title case: first letter of each main word capitalized
        let words: Vec<&str> = line.split_whitespace().collect();
        if words.len() >= 2 {
            let capitalized_count = words
                .iter()
                .filter(|w| w.chars().next().map_or(false, |c| c.is_ascii_uppercase()))
                .count();
            if capitalized_count as f64 / words.len() as f64 > 0.5 {
                return true;
            }
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_tags_simple() {
        let result = HtmlExtractor::strip_tags("<p>Hello <b>world</b></p>");
        assert_eq!(result, "Hello world");
    }

    #[test]
    fn test_strip_tags_with_entities() {
        let result = HtmlExtractor::strip_tags("<p>AT&amp;T &lt; test &gt; &quot;quote&quot;</p>");
        assert_eq!(result, "AT&T < test > \"quote\"");
    }

    #[test]
    fn test_strip_tags_empty() {
        assert_eq!(HtmlExtractor::strip_tags(""), "");
        assert_eq!(HtmlExtractor::strip_tags("<div></div>"), "");
    }

    #[test]
    fn test_detect_sections() {
        let text = "INTRODUCTION TO THE STUDY\nHere is the intro content.\n\nMAIN METHODOLOGY USED HERE\nHere is the method content.\n";
        let sections = HtmlExtractor::detect_sections(text);
        // Should detect at least some sections from ALL CAPS headings
        assert!(
            !sections.is_empty(),
            "should detect headings from ALL CAPS lines"
        );
        assert!(sections.iter().any(|s| s.heading.contains("INTRODUCTION")));
    }

    #[test]
    fn test_detect_sections_colon() {
        let text = "Introduction:\nHere is the intro content.\n\nMethodology:\nHere is the method content.\n";
        let sections = HtmlExtractor::detect_sections(text);
        assert!(
            !sections.is_empty(),
            "should detect headings ending with colon"
        );
    }

    #[test]
    fn test_extract_with_selector() {
        let html = r#"<html><body><article><p>Hello world</p></article></body></html>"#;
        let result = HtmlExtractor::extract(html, "article").unwrap();
        assert!(result.content.contains("Hello world"));
        assert_eq!(result.total_length, result.content.len());
    }

    #[test]
    fn test_extract_fallback_to_body() {
        let html = r#"<html><body><p>Just body content</p></body></html>"#;
        let result = HtmlExtractor::extract(html, "nonexistent").unwrap();
        assert!(result.content.contains("Just body content"));
    }

    #[test]
    fn test_invalid_selector() {
        let html = "<html><body></body></html>";
        let result = HtmlExtractor::extract(html, "]]invalid[[");
        assert!(result.is_err());
    }

    #[test]
    fn test_detect_sections_html() {
        let html = r#"
            <html><body><article>
                <h1>Main Title</h1>
                <p>Introduction content here.</p>
                <h2>First Section</h2>
                <p>Section A details.</p>
                <h2>Second Section</h2>
                <p>Section B details.</p>
            </article></body></html>
        "#;
        let result = HtmlExtractor::extract(html, "article").unwrap();
        assert!(!result.sections.is_empty());
        assert!(result.sections.iter().any(|s| s.heading == "Main Title"));
    }

    #[test]
    fn test_extract_empty_html() {
        let result = HtmlExtractor::extract("", "body").unwrap();
        assert!(result.content.is_empty());
        assert_eq!(result.total_length, 0);
    }
}
