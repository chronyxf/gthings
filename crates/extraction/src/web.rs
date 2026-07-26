use crate::ContentQuality;
use crate::article::{
    Article, ContentTree, ContinuationSignals, ExtractionError, ExtractionInfo, ExtractionMethod,
    QualityScore, Section, SourceInfo,
};
use crate::extractor::{Extractor, SourceType};
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
    fn extract_metadata(&self, doc: &Html, url: &str) -> (SourceInfo, String) {
        let title_sel = Selector::parse("title").ok();
        let meta_author = Selector::parse(r#"meta[name="author"]"#).ok();
        let meta_og_title = Selector::parse(r#"meta[property="og:title"]"#).ok();
        let meta_og_site = Selector::parse(r#"meta[property="og:site_name"]"#).ok();
        let meta_date = Selector::parse(r#"meta[name="date"]"#).ok();
        let meta_article_pub = Selector::parse(r#"meta[property="article:published_time"]"#).ok();
        let html_lang = Selector::parse(r#"html[lang]"#).ok();
        let meta_description = Selector::parse(r#"meta[name="description"]"#).ok();
        let meta_og_desc = Selector::parse(r#"meta[property="og:description"]"#).ok();
        let meta_twitter_desc = Selector::parse(r#"meta[name="twitter:description"]"#).ok();
        let meta_article_tag = Selector::parse(r#"meta[property="article:tag"]"#).ok();
        let meta_article_section = Selector::parse(r#"meta[property="article:section"]"#).ok();
        let meta_dc_creator = Selector::parse(r#"meta[name="dc.creator"]"#).ok();

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

        // Suppress unused variable warnings for selectors parsed for future use
        let _ = (
            meta_description,
            meta_og_desc,
            meta_twitter_desc,
            meta_article_tag,
            meta_article_section,
            meta_dc_creator,
        );

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

    /// Extract sections from full text by finding heading markers.
    fn extract_sections(text: &str) -> Vec<Section> {
        let mut sections = Vec::new();
        let current_depth = 0u8;

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
                } else if trimmed.starts_with('#') {
                    1
                } else {
                    current_depth.clamp(1, 2)
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

    async fn extract(&self, url: String) -> Result<Article, ExtractionError> {
        let start = Instant::now();
        let _source_type = SourceType::from_url(&url);

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| ExtractionError::Http(format!("fetch failed: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
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

        let (source, title) = self.extract_metadata(&doc, &url);

        // Use readability's own HTTP fetch + extraction, fall back to scraper body text
        let readability_text = match readability::extractor::scrape(&url) {
            Ok(article) => article.text,
            Err(_) => {
                let body_sel = Selector::parse("body")
                    .map_err(|_| ExtractionError::Parse("invalid body selector".into()))?;
                doc.select(&body_sel)
                    .next()
                    .map(|e| e.text().collect::<Vec<_>>().join(" "))
                    .unwrap_or_default()
            }
        };

        if readability_text.trim().is_empty() {
            return Err(ExtractionError::Empty(
                "readability produced no content".into(),
            ));
        }

        let sections = Self::extract_sections(&readability_text);

        let quality = Self::score_quality(&readability_text, &sections);

        let total_length = readability_text.len();
        let duration_ms = start.elapsed().as_millis() as u64;

        let signals = ContinuationSignals {
            truncated: false,
            total_length,
            returned_length: total_length,
            is_paywall: quality.reasons.iter().any(|r| r == "paywall_teaser"),
            is_bot_blocked: quality.reasons.iter().any(|r| r == "bot_blocked"),
            is_empty_shell: total_length < 200,
            related_urls: Vec::new(),
        };

        Ok(Article {
            url,
            title,
            source,
            extraction: ExtractionInfo {
                method: ExtractionMethod::Readability,
                confidence: quality.score,
                accessed_at: chrono::Utc::now().to_rfc3339(),
                duration_ms,
            },
            body: ContentTree::Article {
                sections,
                full_text: readability_text,
                total_length,
            },
            signals,
            quality,
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
    fn test_extract_sections_simple() {
        let text =
            "Introduction\nSome text\n## Methods\nDetailed methods\n### Subsection\nMore details";
        let sections = WebExtractor::extract_sections(text);
        assert!(!sections.is_empty());
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
