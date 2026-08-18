/// Extract author and published date from raw JSON-LD script content.
///
/// `chunks` is the inner text of each `<script type="application/ld+json">`
/// block; each chunk is parsed independently and the first author/published
/// values win.
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
                if let Some(a) = item.get("author").and_then(first_name) {
                    author = Some(a);
                }
            }
            if published.is_none() {
                if let Some(d) = item.get("datePublished").and_then(|d| d.as_str()) {
                    published = Some(d.to_string());
                }
            }
            if author.is_none() {
                if let Some(p) = item.get("publisher").and_then(first_name) {
                    author = Some(p);
                }
            }
        }
    }

    (author, published)
}

/// Extract a display name from a JSON-LD value that is either a plain string
/// (e.g. `"author": "John"`) or an object carrying a `name` field (e.g.
/// `{"@type":"Person","name":"Jane Doe"}`). Shared by the `author` and
/// `publisher` branches, which are structurally identical.
fn first_name(val: &serde_json::Value) -> Option<String> {
    if let Some(s) = val.as_str() {
        Some(s.to_string())
    } else if let Some(obj) = val.as_object() {
        obj.get("name").and_then(|n| n.as_str()).map(str::to_string)
    } else {
        None
    }
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
