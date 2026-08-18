//! DOM helpers for single-pass HTML extraction.
//!
//! Owns the `css!` convenience macro for static selectors, the
//! `collect_text_nodes` recursive text collector, and the
//! [`WebExtractor::extract_from_html`] metadata/body/heading/JSON‑LD scrape.

use scraper::{Html, Selector};

use crate::article::SourceInfo;
use crate::extractor::compute_domain_authority;
use crate::jsonld::extract_jsonld;

use super::WebExtractor;

/// Convenience macro for static CSS selectors that are known to be valid.
macro_rules! css {
    ($s:literal) => {
        Selector::parse($s).expect(concat!("valid static selector: ", $s))
    };
}

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

/// Read the `content` attribute of the first element matching `selector`.
///
/// Collapses the repeated `select -> next -> attr("content")` pattern used for
/// `<meta>` tags (author, og:title, og:site_name, date, article:published_time).
fn meta_attr(doc: &Html, selector: &Selector) -> Option<String> {
    doc.select(selector)
        .next()
        .and_then(|el| el.value().attr("content"))
        .map(|s| s.to_string())
}

impl WebExtractor {
    /// Parse `html` once with `scraper` and extract metadata, body text,
    /// heading information, and JSON‑LD from the resulting DOM.
    ///
    /// Returns `(SourceInfo, title, full_text, heading_tuples)`.
    pub(super) fn extract_from_html(
        html: &str,
        url: &str,
    ) -> (SourceInfo, String, String, Vec<(u8, String)>) {
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
        let title = meta_attr(&doc, &sel_meta_og_title)
            .or_else(|| {
                doc.select(&sel_title)
                    .next()
                    .map(|el| el.text().collect::<String>().trim().to_string())
            })
            .unwrap_or_default();

        // ── Metadata from <meta> tags ──
        let author = meta_attr(&doc, &sel_meta_author);

        let site_name = meta_attr(&doc, &sel_meta_og_site).unwrap_or_default();

        let date = meta_attr(&doc, &sel_meta_date);

        let article_pub = meta_attr(&doc, &sel_meta_article_pub);

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
}
