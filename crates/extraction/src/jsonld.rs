/// JSON-LD structured data extraction from HTML pages.
///
/// Parses `<script type="application/ld+json">` blocks to extract
/// author, published date, and other structured metadata.
use scraper::{Html, Selector};

/// Extract author and published date from JSON-LD structured data.
pub fn extract_jsonld(doc: &Html) -> (Option<String>, Option<String>) {
    let sel = Selector::parse(r#"script[type="application/ld+json"]"#).ok();
    let Some(sel) = sel else {
        return (None, None);
    };

    let mut author = None;
    let mut published = None;

    for script in doc.select(&sel) {
        let json_str = script.inner_html();
        if json_str.trim().is_empty() {
            continue;
        }
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&json_str) {
            let items: Vec<&serde_json::Value> =
                if let Some(graph) = val.get("@graph").and_then(|g| g.as_array()) {
                    graph.iter().collect()
                } else {
                    vec![&val]
                };

            for item in &items {
                if author.is_none() {
                    if let Some(a) = item.get("author") {
                        if let Some(s) = a.as_str() {
                            author = Some(s.to_string());
                        } else if let Some(obj) = a.as_object() {
                            if let Some(name) = obj.get("name").and_then(|n| n.as_str()) {
                                author = Some(name.to_string());
                            }
                        }
                    }
                }
                if published.is_none() {
                    if let Some(d) = item.get("datePublished").and_then(|d| d.as_str()) {
                        published = Some(d.to_string());
                    }
                }
                if author.is_none() {
                    if let Some(pub_val) = item.get("publisher") {
                        if let Some(obj) = pub_val.as_object() {
                            if let Some(name) = obj.get("name").and_then(|n| n.as_str()) {
                                author = Some(name.to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    (author, published)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_jsonld_empty() {
        let html = "<html><body></body></html>";
        let doc = Html::parse_document(html);
        let (author, published) = extract_jsonld(&doc);
        assert!(author.is_none());
        assert!(published.is_none());
    }

    #[test]
    fn test_extract_jsonld_article() {
        let html = r#"<html><head><script type="application/ld+json">{"@context":"https://schema.org","@type":"Article","author":{"@type":"Person","name":"Jane Doe"},"datePublished":"2024-01-15"}</script></head><body></body></html>"#;
        let doc = Html::parse_document(html);
        let (author, published) = extract_jsonld(&doc);
        assert_eq!(author.unwrap(), "Jane Doe");
        assert_eq!(published.unwrap(), "2024-01-15");
    }

    #[test]
    fn test_extract_jsonld_graph() {
        let html = r#"<html><head><script type="application/ld+json">{"@graph":[{"@type":"Article","author":"John","datePublished":"2024-03-01"}]}</script></head><body></body></html>"#;
        let doc = Html::parse_document(html);
        let (author, published) = extract_jsonld(&doc);
        assert_eq!(author.unwrap(), "John");
        assert_eq!(published.unwrap(), "2024-03-01");
    }
}
