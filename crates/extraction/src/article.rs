use std::collections::HashSet;

use gthings_common::pagination::Pagination;
use gthings_common::provenance::Provenance;
use serde::{Deserialize, Serialize};

/// The root content type returned by every extractor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Article {
    pub url: String,
    pub title: String,
    pub source: SourceInfo,
    pub extraction: ExtractionInfo,
    pub body: ContentTree,
    pub signals: ContinuationSignals,
    pub quality: QualityScore,
    /// Provenance chain: how this content was discovered/acquired.
    pub provenance: Option<Provenance>,
    /// Pagination state: offset, truncation, continuation token.
    pub pagination: Option<Pagination>,
}

/// Provenance metadata about the source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceInfo {
    pub author: Option<String>,
    pub published: Option<String>, // ISO 8601 date string
    pub site_name: String,
    pub domain_authority: f64, // 0.0-1.0
    pub language: Option<String>,
}

/// How this content was extracted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractionInfo {
    pub method: ExtractionMethod,
    pub confidence: f64,     // 0.0-1.0
    pub accessed_at: String, // ISO 8601
    pub duration_ms: u64,
}

/// The extraction method used.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ExtractionMethod {
    Readability,
    Cetd,
    PdfText,
    RawFile,
    ArxivOai,
    CdpEvaluate,
}

/// The extracted content body — can be structured article, code, or PDF.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ContentTree {
    Article {
        sections: Vec<Section>,
        full_text: String,
        total_length: usize,
    },
    Code {
        language: String,
        content: String,
        file_path: String,
        line_count: usize,
    },
    Pdf {
        pages: usize,
        text: String,
        has_toc: bool,
    },
}

/// A single section/heading within an article.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Section {
    pub heading: String,
    pub depth: u8,       // 1=h1, 2=h2, etc.
    pub offset: usize,   // char offset in full_text
    pub length: usize,   // char length
    pub content: String, // section text
    pub subsections: Vec<Section>,
}

/// AI-agent hints for continuation and quality assessment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContinuationSignals {
    pub truncated: bool,
    pub total_length: usize,
    pub returned_length: usize,
    pub is_paywall: bool,
    pub is_bot_blocked: bool,
    pub is_empty_shell: bool,
    pub related_urls: Vec<String>,
}

/// Content quality assessment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityScore {
    pub score: f64,  // 0.0-1.0
    pub is_ok: bool, // score >= 0.5
    pub reasons: Vec<String>,
    /// Character-level Shannon entropy (bits/char) of extracted text.
    /// High entropy suggests varied/garbled text; low entropy suggests
    /// repetitive/thin content. Set to 0.0 when unavailable.
    #[serde(default)]
    pub entropy_bits_per_char: f32,
}

/// Errors that can occur during extraction.
#[derive(Debug, thiserror::Error)]
pub enum ExtractionError {
    #[error("HTTP error: {0}")]
    Http(String),
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("Unsupported: {0}")]
    Unsupported(String),
    #[error("Empty content: {0}")]
    Empty(String),
    #[error("Bot blocked: {0}")]
    BotBlocked(String),
    #[error("Timeout: {0}")]
    Timeout(String),
}

/// Round a score to 2 decimal places to avoid floating-point artifacts in JSON output.
///
/// f64 arithmetic (e.g. `1.0 - 0.4 - 0.2`) can produce `0.39999999999999997` in JSON.
/// Rounding to 2dp eliminates these artifacts while preserving sufficient precision.
pub(crate) fn round_score(score: f64) -> f64 {
    (score * 100.0).round() / 100.0
}

/// Check if a heading looks like a code artifact rather than a real section title.
fn is_artifact_heading(heading: &str) -> bool {
    let trimmed = heading.trim();
    // Code comments, Rust code artifacts
    if trimmed.contains("//") || trimmed.contains("|") {
        return true;
    }
    // Wikipedia edit links
    if trimmed.contains("[") && trimmed.contains("]") && trimmed.contains("edit") {
        return true;
    }
    // Bare punctuation or bracket artifacts
    if trimmed.starts_with('{')
        || trimmed.starts_with('}')
        || trimmed.starts_with('[')
        || trimmed.starts_with(']')
    {
        return true;
    }
    // HTML/CSS artifacts
    if trimmed.starts_with('.') || trimmed.starts_with('#') || trimmed == "|" {
        return true;
    }
    // Very short headings (1-2 chars) are usually artifacts
    if trimmed.len() <= 2 {
        return true;
    }
    false
}

/// Format an Article as Markdown text for AI agent consumption.
///
/// Produces clean Markdown with:
/// - Title as H1
/// - Metadata as definition list
/// - Sections as H2/H3/H4 with content
/// - Code blocks with language tags
/// - Quality score as footnote
pub fn format_as_markdown(article: &Article) -> String {
    let mut md = String::new();

    // Title
    if !article.title.is_empty() {
        md.push_str(&format!("# {}\n\n", article.title));
    }

    // Metadata block
    if !article.source.site_name.is_empty() {
        md.push_str(&format!("**Source:** {}\n", article.source.site_name));
    }
    if let Some(author) = &article.source.author {
        md.push_str(&format!("**Author:** {author}\n"));
    }
    if let Some(published) = &article.source.published {
        md.push_str(&format!("**Published:** {published}\n"));
    }
    if let Some(lang) = &article.source.language {
        md.push_str(&format!("**Language:** {lang}\n"));
    }
    md.push_str(&format!(
        "**Quality Score:** {:.2}/1.0 — {}\n\n",
        article.quality.score,
        if article.quality.is_ok {
            "pass"
        } else {
            "low quality"
        }
    ));

    // Body content
    match &article.body {
        ContentTree::Article {
            sections,
            full_text,
            ..
        } => {
            if !sections.is_empty() {
                // Render structured sections as headings
                let mut last_heading = String::new();
                let mut seen_headings = HashSet::new();

                for section in sections {
                    let heading = section.heading.trim();
                    if heading.is_empty()
                        || heading == last_heading
                        || seen_headings.contains(heading)
                    {
                        continue;
                    }
                    // Skip artifact headings that are code fragments, not real titles
                    if is_artifact_heading(heading) {
                        continue;
                    }
                    seen_headings.insert(heading.to_string());
                    last_heading = heading.to_string();

                    // Determine heading level based on depth
                    let level = match section.depth {
                        0..=1 => 2, // H2 for top-level
                        2 => 3,     // H3
                        _ => 4,     // H4+
                    };
                    let prefix = "#".repeat(level);
                    md.push_str(&format!("{prefix} {heading}\n\n"));

                    // Add section content
                    let content = section.content.trim();
                    if !content.is_empty() {
                        md.push_str(content);
                        md.push_str("\n\n");
                    }
                }
            } else if !full_text.is_empty() {
                // No sections — use full text as-is
                md.push_str(full_text);
                md.push('\n');
            }
        }
        ContentTree::Code {
            language,
            content,
            file_path,
            line_count,
        } => {
            if !file_path.is_empty() {
                md.push_str(&format!("**File:** `{file_path}`\n"));
            }
            md.push_str(&format!("**Language:** {language}\n"));
            md.push_str(&format!("**Lines:** {line_count}\n\n"));

            md.push_str(&format!("```{language}\n{content}\n```\n"));
        }
        ContentTree::Pdf {
            pages,
            text,
            has_toc,
        } => {
            md.push_str(&format!("**Pages:** {pages}\n"));
            if *has_toc {
                md.push_str("**Table of Contents:** Yes\n");
            }
            md.push_str(&format!("\n{}", text));
            md.push('\n');
        }
    }

    // Signals / warnings
    if article.signals.is_empty_shell {
        md.push_str("\n> [!] **Warning:** This content appears to be an empty shell (navigation chrome only).\n");
    }
    if article.signals.is_paywall {
        md.push_str("\n> [LOCKED] **Warning:** Paywall detected — content may be incomplete.\n");
    }
    if article.signals.truncated {
        md.push_str(&format!(
            "\n> [TRUNCATED] **Note:** Content truncated at {} chars (total: {}).\n",
            article.signals.returned_length, article.signals.total_length
        ));
    }

    // Quality reasons
    if !article.quality.reasons.is_empty() {
        md.push_str("\n---\n*Quality flags:* ");
        for (i, reason) in article.quality.reasons.iter().enumerate() {
            if i > 0 {
                md.push_str(", ");
            }
            md.push_str(&format!("`{reason}`"));
        }
        md.push('\n');
    }

    md
}
