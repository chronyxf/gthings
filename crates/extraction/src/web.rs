use std::time::Instant;

use async_trait::async_trait;
use gthings_common::pagination::ExtractParams;
use gthings_common::provenance::{ExtractionMethod as ProvenanceMethod, Provenance};
use scraper::{Html, Selector};

use crate::ContentQuality;
use crate::article::{
    Article, ContentTree, ContinuationSignals, ExtractionError, ExtractionInfo, ExtractionMethod,
    QualityScore, Section, SourceInfo,
};
use crate::extractor::{Extractor, compute_domain_authority};
use crate::jsonld::extract_jsonld;

#[cfg(test)]
use crate::extractor::SourceType;

/// Convenience macro for static CSS selectors that are known to be valid.
macro_rules! css {
    ($s:literal) => {
        Selector::parse($s).expect(concat!("valid static selector: ", $s))
    };
}

// ---------------------------------------------------------------------------
// Internal helpers for single-pass DOM extraction
// ---------------------------------------------------------------------------

/// Collect all text nodes from an element's subtree, skipping non-content
/// regions (`<nav>`, `<header>`, `<footer>`, `<aside>`, `<form>`, `<script>`,
/// `<style>`).
fn collect_text_nodes(el: &scraper::ElementRef, parts: &mut Vec<String>) {
    let tag_name = el.value().name();

    // Skip non-content regions entirely
    if matches!(
        tag_name,
        "nav" | "header" | "footer" | "aside" | "form" | "script" | "style"
    ) {
        return;
    }

    for child in el.children() {
        match child.value() {
            scraper::node::Node::Text(text) => {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    parts.push(trimmed.to_string());
                }
            }
            scraper::node::Node::Element(_) => {
                if let Some(child_el) = scraper::ElementRef::wrap(child) {
                    collect_text_nodes(&child_el, parts);
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Extracts content from HTML web pages using a single-pass `scraper`-based
/// DOM parse (html5ever), eliminating the double‑parse overhead of the
/// previous `scraper` + `readability` pipeline.
pub struct WebExtractor {
    client: reqwest::Client,
}

impl WebExtractor {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }

    // ------------------------------------------------------------------
    // Single-pass extraction via scraper DOM
    // ------------------------------------------------------------------

    /// Parse `html` once with `scraper` and extract metadata, body text,
    /// heading information, and JSON‑LD from the resulting DOM.
    ///
    /// Returns `(SourceInfo, title, full_text, heading_tuples)`.
    fn extract_from_html(html: &str, url: &str) -> (SourceInfo, String, String, Vec<(u8, String)>) {
        let doc = Html::parse_document(html);

        // ── Selectors ──
        let sel_title = css!("title");
        let sel_meta_author = css!(r#"meta[name="author"]"#);
        let sel_meta_og_title = css!(r#"meta[property="og:title"]"#);
        let sel_meta_og_site = css!(r#"meta[property="og:site_name"]"#);
        let sel_meta_date = css!(r#"meta[name="date"]"#);
        let sel_meta_article_pub = css!(r#"meta[property="article:published_time"]"#);
        let sel_html = css!("html");
        let sel_jsonld = css!(r#"script[type="application/ld+json"]"#);

        // ── Title: prefer og:title over <title> ──
        let title = doc
            .select(&sel_meta_og_title)
            .next()
            .and_then(|el| el.value().attr("content"))
            .map(|s| s.to_string())
            .or_else(|| {
                doc.select(&sel_title)
                    .next()
                    .map(|el| el.text().collect::<String>().trim().to_string())
            })
            .unwrap_or_default();

        // ── Metadata from <meta> tags ──
        let author = doc
            .select(&sel_meta_author)
            .next()
            .and_then(|el| el.value().attr("content"))
            .map(|s| s.to_string());

        let site_name = doc
            .select(&sel_meta_og_site)
            .next()
            .and_then(|el| el.value().attr("content"))
            .map(|s| s.to_string())
            .unwrap_or_default();

        let date = doc
            .select(&sel_meta_date)
            .next()
            .and_then(|el| el.value().attr("content"))
            .map(|s| s.to_string());

        let article_pub = doc
            .select(&sel_meta_article_pub)
            .next()
            .and_then(|el| el.value().attr("content"))
            .map(|s| s.to_string());

        let language = doc
            .select(&sel_html)
            .next()
            .and_then(|el| el.value().attr("lang"))
            .map(|s| s.to_string());

        // ── JSON‑LD ──
        let jsonld_chunks: Vec<String> = doc
            .select(&sel_jsonld)
            .filter_map(|el| {
                let text: String = el.text().collect();
                let trimmed = text.trim().to_string();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed)
                }
            })
            .collect();

        // ── Body text (single recursive pass skipping non-content regions) ──
        let sel_body = css!("body");
        let mut text_parts: Vec<String> = Vec::new();
        if let Some(body) = doc.select(&sel_body).next() {
            collect_text_nodes(&body, &mut text_parts);
        }
        let full_text = text_parts.join(" ");

        // ── Headings ──
        let sel_headings = css!("h1, h2, h3, h4, h5, h6");
        let headings: Vec<(u8, String)> = doc
            .select(&sel_headings)
            .filter_map(|el| {
                let tag = el.value().name();
                let depth = tag
                    .get(1..)
                    .and_then(|n| n.parse::<u8>().ok())
                    .filter(|&d| (1..=6).contains(&d))?;
                let text = el.text().collect::<String>();
                let trimmed = text.trim().to_string();
                if trimmed.is_empty() {
                    None
                } else {
                    Some((depth, trimmed))
                }
            })
            .collect();

        // JSON‑LD structured data
        let (jsonld_author, jsonld_published) = extract_jsonld(&jsonld_chunks);

        let source = SourceInfo {
            author: author.or(jsonld_author),
            published: article_pub.or(date).or(jsonld_published),
            site_name,
            domain_authority: compute_domain_authority(url),
            language,
        };

        (source, title, full_text, headings)
    }

    // ------------------------------------------------------------------
    // Sections from headings
    // ------------------------------------------------------------------

    /// Build a nested section tree from a list of heading `(depth, text)`
    /// pairs extracted during the streaming pass.  This mirrors the logic
    /// previously done by `extract_sections_from_html` but operates on the
    /// pre‑extracted heading data instead of a full DOM.
    fn build_sections_from_headings(headings: &[(u8, String)], full_text: &str) -> Vec<Section> {
        if headings.is_empty() {
            return Self::extract_sections(full_text);
        }

        // Map headings to offsets within full_text via incremental search
        let mut mapped: Vec<(u8, String, usize)> = Vec::new();
        let mut search_from = 0usize;
        for (depth, text) in headings {
            if let Some(offset) = full_text[search_from..].find(text.as_str()) {
                let abs = search_from + offset;
                mapped.push((*depth, text.clone(), abs));
                search_from = abs + text.len();
            }
        }

        if mapped.is_empty() {
            return Self::extract_sections(full_text);
        }

        // Build flat sections with body text between consecutive headings
        let mut flat: Vec<Section> = Vec::with_capacity(mapped.len());
        for i in 0..mapped.len() {
            let (depth, ref heading, offset) = mapped[i];
            let end = if i + 1 < mapped.len() {
                mapped[i + 1].2
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

        Self::build_section_tree(flat)
    }

    // ------------------------------------------------------------------
    // Flat-text heading heuristic (fallback)
    // ------------------------------------------------------------------

    /// Extract sections from full text by finding heading markers
    /// (flat heuristic fallback).
    ///
    /// Uses a single‑pass line scan that tracks byte positions to avoid
    /// O(n·m) `str::find` for each detected heading.
    fn extract_sections(text: &str) -> Vec<Section> {
        let mut sections = Vec::new();
        let mut byte_pos: usize = 0;

        for line in text.split('\n') {
            let line_len = line.len();
            let trimmed = line.trim();
            let trimmed_len = trimmed.len();

            // Detect heading-like lines: markdown `##` or ALL-CAPS lines
            let is_heading = trimmed.starts_with('#')
                || (trimmed_len > 3
                    && trimmed_len < 100
                    && trimmed.chars().all(|c| {
                        c.is_uppercase() || c.is_whitespace() || c.is_ascii_punctuation()
                    }));

            if is_heading {
                let depth = if trimmed.starts_with("###") {
                    3
                } else if trimmed.starts_with("##") {
                    2
                } else {
                    1u8
                };

                let heading_text = if trimmed.starts_with('#') {
                    trimmed.trim_matches('#').trim().to_string()
                } else {
                    trimmed.to_string()
                };

                // Compute offset: trimmed text starts after any leading whitespace
                let leading_ws = line.len() - line.trim_start().len();
                let offset = byte_pos + leading_ws;

                sections.push(Section {
                    heading: heading_text,
                    depth,
                    offset,
                    length: 0,
                    content: String::new(),
                    subsections: Vec::new(),
                });
            }

            byte_pos += line_len + 1; // +1 for the '\n' separator
        }

        // Fill content for each section from its offset to the next section's offset
        for i in 0..sections.len() {
            let end = if i + 1 < sections.len() {
                sections[i + 1].offset
            } else {
                text.len()
            };
            sections[i].length = end.saturating_sub(sections[i].offset);
            let content_end = sections[i].offset + sections[i].length;
            if content_end <= text.len() {
                sections[i].content = text[sections[i].offset..content_end].to_string();
            }
        }

        sections
    }

    // ------------------------------------------------------------------
    // Section tree building (shared by both heading sources)
    // ------------------------------------------------------------------

    /// Convert a flat list of offset-sorted sections into a nested tree
    /// using heading depth (HTML outline algorithm).
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

    fn insert_into_sections(sections: &mut Vec<Section>, section: Section, depth: u8) {
        // Grab a single mutable reference to the last section; if the Vec is
        // unexpectedly empty, push the new section as root and return early.
        let Some(last) = sections.last_mut() else {
            sections.push(section);
            return;
        };
        let last_depth = last.depth;

        if last_depth < depth {
            let recurse = last
                .subsections
                .last()
                .is_some_and(|child| child.depth < depth);

            if recurse {
                Self::insert_into_sections(&mut last.subsections, section, depth);
            } else {
                last.subsections.push(section);
            }
        } else if !last.subsections.is_empty() {
            Self::insert_into_sections(&mut last.subsections, section, depth);
        } else {
            sections.push(section);
        }
    }

    // ------------------------------------------------------------------
    // Quality scoring
    // ------------------------------------------------------------------

    fn score_quality(text: &str, sections: &[Section]) -> QualityScore {
        let result = ContentQuality::validate(text);
        let mut score = result.score;
        let mut reasons: Vec<String> = result
            .reasons
            .into_iter()
            .map(|r| format!("{:?}", r))
            .collect();

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
}

// ---------------------------------------------------------------------------
// Extractor trait impl
// ---------------------------------------------------------------------------

#[async_trait]
impl Extractor for WebExtractor {
    type Input = String;

    async fn extract(
        &self,
        url: String,
        params: ExtractParams,
    ) -> Result<Article, ExtractionError> {
        let start = Instant::now();

        // ── Fetch ──
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| ExtractionError::Http(format!("fetch failed: {e}")))?;

        if !resp.status().is_success() {
            crate::dispatch::check_rate_limit(&resp, format!("Rate limited while fetching {url}"))?;
            return Err(ExtractionError::Http(format!(
                "HTTP {} for URL {}",
                resp.status(),
                url
            )));
        }

        let html = resp
            .text()
            .await
            .map_err(|e| ExtractionError::Http(format!("read body: {e}")))?;

        if html.len() < 100 {
            return Err(ExtractionError::Empty("response too short".into()));
        }

        // ── Single‑pass extraction ──
        let (source, title, full_text, headings) = Self::extract_from_html(&html, &url);

        if full_text.trim().is_empty() {
            return Err(ExtractionError::Empty(
                "extraction produced no content".into(),
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
            gthings_common::pagination::build_pagination(&params, &url, total_len, effective_len)
                .map_err(|e| ExtractionError::Parse(e.to_string()))?;

        // Build sections from the streaming‑extracted headings
        let sections = Self::build_sections_from_headings(&headings, &effective_text);

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
            agent: gthings_common::GTHINGS_AGENT.into(),
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ── Quality scoring ──

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
        let text = crate::quality::READ_MORE_INDICATOR.to_string();
        let q = WebExtractor::score_quality(&text, &[]);
        assert!(!q.is_ok);
        assert!(q.reasons.contains(&"PaywallTeaser".to_string()));
        assert!(q.reasons.contains(&"no_headings".to_string()));
    }

    // ── Section extraction from flat text ──

    #[test]
    fn test_extract_sections_fallback() {
        let text =
            "Introduction\nSome text\n## Methods\nDetailed methods\n### Subsection\nMore details";
        let sections = WebExtractor::extract_sections(text);
        assert!(!sections.is_empty());
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].heading, "Methods");
        assert_eq!(sections[0].depth, 2);
    }

    // ── Section building from heading tuples ──

    #[test]
    fn test_build_sections_from_headings_nested() {
        let full_text = "Main Title Some intro text here. First Section Body of first section. Sub Section A Deeper body. Second Section Last section body.";
        let headings = vec![
            (1u8, "Main Title".to_string()),
            (2u8, "First Section".to_string()),
            (3u8, "Sub Section A".to_string()),
            (2u8, "Second Section".to_string()),
        ];

        let sections = WebExtractor::build_sections_from_headings(&headings, full_text);
        assert_eq!(sections.len(), 1, "should have one root h1 section");
        assert_eq!(sections[0].heading, "Main Title");
        assert_eq!(sections[0].depth, 1);
        assert_eq!(
            sections[0].subsections.len(),
            2,
            "h1 should contain two h2 subsections"
        );
        assert_eq!(sections[0].subsections[0].heading, "First Section");
        assert_eq!(sections[0].subsections[0].depth, 2);
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
    fn test_build_sections_from_headings_empty_fallback() {
        // No headings → should fall back to flat heuristic (which also
        // finds nothing in this text) and return empty.
        let full_text = "No headings here. Just paragraphs.";
        let headings: Vec<(u8, String)> = vec![];
        let sections = WebExtractor::build_sections_from_headings(&headings, full_text);
        assert!(sections.is_empty(), "no headings → empty sections");
    }

    #[test]
    fn test_build_sections_from_headings_sibling_h2s() {
        // Two h2s at root level (no h1) should both appear at root
        let full_text = "Part A Content A Part B Content B";
        let headings = vec![(2u8, "Part A".to_string()), (2u8, "Part B".to_string())];
        let sections = WebExtractor::build_sections_from_headings(&headings, full_text);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].heading, "Part A");
        assert_eq!(sections[1].heading, "Part B");
        assert_eq!(sections[0].subsections.len(), 0);
        assert_eq!(sections[1].subsections.len(), 0);
    }

    // ── Source type detection (unchanged) ──

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
