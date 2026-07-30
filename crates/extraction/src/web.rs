use gthings_common::pagination::ExtractParams;
use gthings_common::provenance::{ExtractionMethod as ProvenanceMethod, Provenance};

use crate::ContentQuality;
use crate::article::{
    Article, ContentTree, ContinuationSignals, ExtractionError, ExtractionInfo, ExtractionMethod,
    QualityScore, Section, SourceInfo,
};
use crate::extractor::Extractor;
#[cfg(test)]
use crate::extractor::SourceType;
use async_trait::async_trait;
use scraper::{Html, Selector};
use std::time::Instant;

/// Extracts content from HTML web pages.
/// Uses Readability for main article extraction + scraper for metadata/sections.
pub struct WebExtractor {
    client: reqwest::Client,
}

impl WebExtractor {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }

    /// Extract metadata from HTML <head> and <meta> tags.
    /// Returns (SourceInfo, title).
    fn extract_metadata(doc: &Html, url: &str) -> (SourceInfo, String) {
        let title_sel = Selector::parse("title").ok();
        let meta_author = Selector::parse(r#"meta[name="author"]"#).ok();
        let meta_og_title = Selector::parse(r#"meta[property="og:title"]"#).ok();
        let meta_og_site = Selector::parse(r#"meta[property="og:site_name"]"#).ok();
        let meta_date = Selector::parse(r#"meta[name="date"]"#).ok();
        let meta_article_pub = Selector::parse(r#"meta[property="article:published_time"]"#).ok();
        let html_lang = Selector::parse(r#"html[lang]"#).ok();

        let title = meta_og_title
            .as_ref()
            .and_then(|s| doc.select(s).next())
            .and_then(|e| e.value().attr("content"))
            .map(|s| s.to_string())
            .or_else(|| {
                title_sel
                    .as_ref()
                    .and_then(|s| doc.select(s).next())
                    .map(|e| e.inner_html())
            })
            .unwrap_or_default();

        let site_name = meta_og_site
            .as_ref()
            .and_then(|s| doc.select(s).next())
            .and_then(|e| e.value().attr("content"))
            .unwrap_or("")
            .to_string();

        let author = meta_author
            .as_ref()
            .and_then(|s| doc.select(s).next())
            .and_then(|e| e.value().attr("content"))
            .map(|s| s.to_string());

        let published = meta_article_pub
            .as_ref()
            .or(meta_date.as_ref())
            .and_then(|s| doc.select(s).next())
            .and_then(|e| e.value().attr("content"))
            .map(|s| s.to_string());

        let language = html_lang
            .as_ref()
            .and_then(|s| doc.select(s).next())
            .and_then(|e| e.value().attr("lang"))
            .map(|s| s.to_string());
        // Parse JSON-LD metadata
        let (jsonld_author, jsonld_published) = Self::extract_jsonld(doc);

        (
            SourceInfo {
                author: author.or(jsonld_author),
                published: published.or(jsonld_published),
                site_name,
                domain_authority: crate::extractor::compute_domain_authority(url),
                language,
            },
            title,
        )
    }

    /// Extract sections from full text by finding heading markers (flat heuristic fallback).
    fn extract_sections(text: &str) -> Vec<Section> {
        let mut sections = Vec::new();
        for line in text.lines() {
            let trimmed = line.trim();
            let is_heading = trimmed.starts_with('#')
                || trimmed
                    .chars()
                    .all(|c| c.is_uppercase() || c.is_whitespace() || c.is_ascii_punctuation())
                    && trimmed.len() < 100
                    && trimmed.len() > 3;

            if is_heading && trimmed.len() < 100 {
                let depth = if trimmed.starts_with("###") {
                    3
                } else if trimmed.starts_with("##") {
                    2
                } else {
                    1u8
                };

                let offset = text.find(trimmed).unwrap_or(0);

                sections.push(Section {
                    heading: trimmed.trim_matches('#').trim().to_string(),
                    depth,
                    offset,
                    length: 0,
                    content: String::new(),
                    subsections: Vec::new(),
                });
            }
        }

        for i in 0..sections.len() {
            let end = if i + 1 < sections.len() {
                sections[i + 1].offset
            } else {
                text.len()
            };
            sections[i].length = end.saturating_sub(sections[i].offset);
            if sections[i].offset + sections[i].length <= text.len() {
                sections[i].content =
                    text[sections[i].offset..sections[i].offset + sections[i].length].to_string();
            }
        }

        sections
    }

    /// Extract sections from an HTML document by walking the DOM heading hierarchy.
    ///
    /// Uses `scraper` to select h1–h6 elements in document order, determines
    /// section body text from `full_text`, and builds a proper nested tree.
    /// Falls back to [`extract_sections`] when no heading elements exist.
    fn extract_sections_from_html(document: &Html, full_text: &str) -> Vec<Section> {
        let selector = match Selector::parse("h1, h2, h3, h4, h5, h6") {
            Ok(s) => s,
            Err(_) => return Self::extract_sections(full_text),
        };

        // Collect heading metadata from DOM
        let mut raw_headings: Vec<(u8, String)> = Vec::new();
        for element in document.select(&selector) {
            let tag = element.value().name();
            let depth = match tag {
                "h1" => 1,
                "h2" => 2,
                "h3" => 3,
                "h4" => 4,
                "h5" => 5,
                "h6" => 6,
                _ => continue,
            };
            let heading_text: String = element.text().collect::<Vec<_>>().concat();
            let heading_text = heading_text.trim().to_string();
            if heading_text.is_empty() {
                continue;
            }
            raw_headings.push((depth, heading_text));
        }

        if raw_headings.is_empty() {
            return Self::extract_sections(full_text);
        }

        // Map headings to offsets within full_text, searching incrementally
        let mut headings: Vec<(u8, String, usize)> = Vec::new();
        let mut search_from = 0usize;
        for (depth, text) in &raw_headings {
            if let Some(offset) = full_text[search_from..].find(text.as_str()) {
                let absolute_offset = search_from + offset;
                headings.push((*depth, text.clone(), absolute_offset));
                search_from = absolute_offset + text.len();
            }
        }

        if headings.is_empty() {
            return Self::extract_sections(full_text);
        }

        // Build flat sections with body text between consecutive headings
        let mut flat: Vec<Section> = Vec::with_capacity(headings.len());
        for i in 0..headings.len() {
            let (depth, ref heading, offset) = headings[i];
            let end = if i + 1 < headings.len() {
                headings[i + 1].2
            } else {
                full_text.len()
            };
            let length = end.saturating_sub(offset);
            let content = full_text[offset..end].to_string();
            flat.push(Section {
                heading: heading.clone(),
                depth,
                offset,
                length,
                content,
                subsections: Vec::new(),
            });
        }

        // Nest into proper heading hierarchy
        Self::build_section_tree(flat)
    }

    /// Convert a flat list of offset-sorted sections into a nested tree using
    /// heading depth (HTML outline algorithm).
    fn build_section_tree(flat: Vec<Section>) -> Vec<Section> {
        let mut roots: Vec<Section> = Vec::new();
        for section in flat {
            let depth = section.depth;
            if roots.is_empty() {
                roots.push(section);
            } else {
                Self::insert_into_sections(&mut roots, section, depth);
            }
        }
        roots
    }

    /// Insert `section` of given `depth` into the tree rooted at `sections`
    /// using the HTML outline algorithm:
    /// - Walk down the last child chain as long as depth keeps increasing
    /// - The first section with depth < current depth becomes the parent
    fn insert_into_sections(sections: &mut Vec<Section>, section: Section, depth: u8) {
        // sections is guaranteed non-empty by the caller
        let last_depth = sections
            .last()
            .expect("sections non-empty after checks")
            .depth;

        if last_depth < depth {
            // Potential parent found — check if its last child is a closer parent
            let recurse = sections
                .last()
                .and_then(|s| s.subsections.last())
                .is_some_and(|child| child.depth < depth);

            if recurse {
                Self::insert_into_sections(
                    &mut sections
                        .last_mut()
                        .expect("sections non-empty after checks")
                        .subsections,
                    section,
                    depth,
                );
            } else {
                sections
                    .last_mut()
                    .expect("sections non-empty after checks")
                    .subsections
                    .push(section);
            }
        } else if !sections
            .last()
            .expect("sections non-empty after checks")
            .subsections
            .is_empty()
        {
            // Recurse into subsections — last_child.depth >= depth so we go deeper
            Self::insert_into_sections(
                &mut sections
                    .last_mut()
                    .expect("sections non-empty after checks")
                    .subsections,
                section,
                depth,
            );
        } else {
            // Same level or deeper — add as sibling
            sections.push(section);
        }
    }

    /// Score content quality based on length, structure, and patterns.
    ///
    /// Delegates to [`ContentQuality::validate`] for the core scoring, then
    /// applies web-specific heuristics (section count).
    fn score_quality(text: &str, sections: &[Section]) -> QualityScore {
        let result = ContentQuality::validate(text);
        let mut score = result.score;
        let mut reasons: Vec<String> = result
            .reasons
            .into_iter()
            .map(|r| format!("{:?}", r))
            .collect();

        // Section check (specific to web extraction)
        if sections.len() < 2 {
            score -= 0.1;
            reasons.push("no_headings".into());
        }

        let score = crate::article::round_score(score.clamp(0.0, 1.0));
        QualityScore {
            score,
            is_ok: score >= 0.5,
            reasons,
            entropy_bits_per_char: result.entropy_bits_per_char,
        }
    }

    /// Extract author and published date from JSON-LD structured data.
    fn extract_jsonld(doc: &Html) -> (Option<String>, Option<String>) {
        crate::jsonld::extract_jsonld(doc)
    }
}

#[async_trait]
impl Extractor for WebExtractor {
    type Input = String;

    async fn extract(
        &self,
        url: String,
        params: ExtractParams,
    ) -> Result<Article, ExtractionError> {
        let start = Instant::now();
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| ExtractionError::Http(format!("fetch failed: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            if status.as_u16() == 429 {
                let retry_after = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok());
                return Err(ExtractionError::RateLimited {
                    detail: format!("Rate limited while fetching {url}"),
                    retry_after,
                });
            }
            return Err(ExtractionError::Http(format!("HTTP {status}")));
        }

        let html = resp
            .text()
            .await
            .map_err(|e| ExtractionError::Http(format!("read body: {e}")))?;

        if html.len() < 100 {
            return Err(ExtractionError::Empty("response too short".into()));
        }

        let doc = Html::parse_document(&html);

        let (source, title) = Self::extract_metadata(&doc, &url);

        // Use readability's local extraction on already-fetched HTML (no double fetch)
        let full_text = {
            let parsed_url = url::Url::parse(&url)
                .map_err(|e| ExtractionError::Parse(format!("invalid URL: {e}")))?;
            let mut slice: &[u8] = html.as_bytes();
            match readability::extractor::extract(&mut slice, &parsed_url) {
                Ok(article) => article.text,
                Err(_) => {
                    let body_sel = Selector::parse("body")
                        .map_err(|_| ExtractionError::Parse("invalid body selector".into()))?;
                    doc.select(&body_sel)
                        .next()
                        .map(|e| e.text().collect::<Vec<_>>().join(" "))
                        .unwrap_or_default()
                }
            }
        };

        if full_text.trim().is_empty() {
            return Err(ExtractionError::Empty(
                "readability produced no content".into(),
            ));
        }

        let total_len = full_text.len();

        // Apply offset and max_chars slicing
        let effective_text: String = full_text
            .chars()
            .skip(params.offset)
            .take(params.max_chars)
            .collect();

        let effective_len = effective_text.len();

        let pagination =
            gthings_common::pagination::build_pagination(&params, &url, total_len, effective_len);

        let sections = Self::extract_sections_from_html(&doc, &effective_text);

        let quality = Self::score_quality(&effective_text, &sections);

        let duration_ms = start.elapsed().as_millis() as u64;

        let signals = ContinuationSignals {
            truncated: pagination.truncated,
            total_length: total_len,
            returned_length: effective_len,
            is_paywall: quality.reasons.iter().any(|r| r == "paywall_teaser"),
            is_bot_blocked: quality.reasons.iter().any(|r| r == "bot_blocked"),
            is_empty_shell: total_len < 200,
            related_urls: Vec::new(),
        };

        let now = chrono::Utc::now();
        let provenance = Provenance {
            source_url: url.clone(),
            method: ProvenanceMethod::Readability,
            agent: "gthings-web-extractor".into(),
            accessed_at: now,
            duration_ms,
            derived_from: None,
        };

        Ok(Article {
            url,
            title,
            source,
            extraction: ExtractionInfo {
                method: ExtractionMethod::Readability,
                confidence: quality.score,
                accessed_at: now.to_rfc3339(),
                duration_ms,
            },
            body: ContentTree::Article {
                sections,
                full_text: effective_text,
                total_length: total_len,
            },
            signals,
            quality,
            provenance: Some(provenance),
            pagination: Some(pagination),
        })
    }

    fn method(&self) -> ExtractionMethod {
        ExtractionMethod::Readability
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_score_quality_empty() {
        let q = WebExtractor::score_quality("", &[]);
        assert!(!q.is_ok);
        assert!(q.reasons.contains(&"EmptyContent".to_string()));
        assert!(q.reasons.contains(&"no_headings".to_string()));
    }

    #[test]
    fn test_score_quality_good() {
        let text = "A".repeat(500);
        let sections = vec![
            Section {
                heading: "Intro".into(),
                depth: 1,
                offset: 0,
                length: 100,
                content: "a".into(),
                subsections: vec![],
            },
            Section {
                heading: "Body".into(),
                depth: 2,
                offset: 100,
                length: 300,
                content: "b".into(),
                subsections: vec![],
            },
        ];
        let q = WebExtractor::score_quality(&text, &sections);
        assert!(q.is_ok);
        assert!(q.score > 0.5);
    }

    #[test]
    fn test_score_quality_paywall() {
        // ContentQuality flags "Read More »" as PaywallTeaser; web layer adds no_headings
        let text = "Read More \u{00bb}".to_string();
        let q = WebExtractor::score_quality(&text, &[]);
        assert!(!q.is_ok);
        assert!(q.reasons.contains(&"PaywallTeaser".to_string()));
        assert!(q.reasons.contains(&"no_headings".to_string()));
    }

    #[test]
    fn test_extract_sections_fallback() {
        // Flat-text heuristic is preserved as fallback for headingless pages
        let text =
            "Introduction\nSome text\n## Methods\nDetailed methods\n### Subsection\nMore details";
        let sections = WebExtractor::extract_sections(text);
        assert!(!sections.is_empty());
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].heading, "Methods");
        assert_eq!(sections[0].depth, 2);
    }

    #[test]
    fn test_extract_sections_from_html_nested() {
        let html = r#"
<!DOCTYPE html>
<html><body>
<h1>Main Title</h1>
<p>Some intro text here.</p>
<h2>First Section</h2>
<p>Body of first section.</p>
<h3>Sub Section A</h3>
<p>Deeper body.</p>
<h2>Second Section</h2>
<p>Last section body.</p>
</body></html>
"#;
        let doc = Html::parse_document(html);
        // Full_text is a simulated Readability-like flat text
        let full_text = "Main Title Some intro text here. First Section Body of first section. Sub Section A Deeper body. Second Section Last section body.";

        let sections = WebExtractor::extract_sections_from_html(&doc, full_text);
        assert_eq!(sections.len(), 1, "should have one root h1 section");
        assert_eq!(sections[0].heading, "Main Title");
        assert_eq!(sections[0].depth, 1);
        // The h1 section spans from "Main Title" to end of doc (since it's the root)
        // Its subsections should include the two h2 sections
        assert_eq!(
            sections[0].subsections.len(),
            2,
            "h1 should contain two h2 subsections"
        );
        assert_eq!(sections[0].subsections[0].heading, "First Section");
        assert_eq!(sections[0].subsections[0].depth, 2);
        // First Section should have one h3 subsection
        assert_eq!(
            sections[0].subsections[0].subsections.len(),
            1,
            "h2 'First Section' should contain one h3"
        );
        assert_eq!(
            sections[0].subsections[0].subsections[0].heading,
            "Sub Section A"
        );
        assert_eq!(sections[0].subsections[0].subsections[0].depth, 3);
        assert_eq!(sections[0].subsections[1].heading, "Second Section");
    }

    #[test]
    fn test_extract_sections_from_html_headingless_fallback() {
        // No h1-h6 elements → should fall back to flat heuristic without panicking
        let html = r#"<html><body><p>No headings here.</p><p>Just paragraphs.</p></body></html>"#;
        let doc = Html::parse_document(html);
        let full_text = "No headings here. Just paragraphs.";
        // Should not panic and return empty sections (since flat heuristic finds no headings either)
        let sections = WebExtractor::extract_sections_from_html(&doc, full_text);
        assert!(sections.is_empty(), "no headings → empty sections");
    }

    #[test]
    fn test_extract_sections_from_html_sibling_h2s() {
        // Two h2s at root level (no h1) should both appear at root
        let html = r#"<html><body><h2>Part A</h2><p>Content A</p><h2>Part B</h2><p>Content B</p></body></html>"#;
        let doc = Html::parse_document(html);
        let full_text = "Part A Content A Part B Content B";
        let sections = WebExtractor::extract_sections_from_html(&doc, full_text);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].heading, "Part A");
        assert_eq!(sections[1].heading, "Part B");
        assert_eq!(sections[0].subsections.len(), 0);
        assert_eq!(sections[1].subsections.len(), 0);
    }

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
