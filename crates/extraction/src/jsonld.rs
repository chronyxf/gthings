/// JSON-LD structured data extraction.
///
/// Parses raw JSON-LD strings (from `<script type="application/ld+json">` blocks)
/// to extract author, published date, and other structured metadata.
/// Accepts the combined inner text of all JSON-LD script blocks
/// (each chunk is parsed separately and results are unioned).
///
/// Extract author and published date from raw JSON-LD script content.
/// `chunks` is the inner text of each `<script type="application/ld+json">` block.
pub fn extract_jsonld(chunks: &[String]) -> (Option<String>, Option<String>) {
    let mut author = None;
    let mut published = None;

    for json_str in chunks {
        let trimmed = json_str.trim();
        if trimmed.is_empty() {
            continue;
        }
        let val = match serde_json::from_str::<serde_json::Value>(trimmed) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "JSON-LD parse failed");
                continue;
            }
        };
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

    (author, published)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_jsonld_empty() {
        let chunks: Vec<String> = vec!["".to_string()];
        let (author, published) = extract_jsonld(&chunks);
        assert!(author.is_none());
        assert!(published.is_none());
    }

    #[test]
    fn test_extract_jsonld_article() {
        let chunks = vec![r#"{"@context":"https://schema.org","@type":"Article","author":{"@type":"Person","name":"Jane Doe"},"datePublished":"2024-01-15"}"#.to_string()];
        let (author, published) = extract_jsonld(&chunks);
        assert_eq!(author.unwrap(), "Jane Doe");
        assert_eq!(published.unwrap(), "2024-01-15");
    }

    #[test]
    fn test_extract_jsonld_graph() {
        let chunks = vec![
            r#"{"@graph":[{"@type":"Article","author":"John","datePublished":"2024-03-01"}]}"#
                .to_string(),
        ];
        let (author, published) = extract_jsonld(&chunks);
        assert_eq!(author.unwrap(), "John");
        assert_eq!(published.unwrap(), "2024-03-01");
    }
}
