use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Method used to discover or acquire the content.
///
/// Distinct from the extraction technique (e.g. Readability vs PdfText);
/// this describes *how the agent found the content*.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub enum ExtractionMethod {
    #[default]
    Follow,
    Search,
    Readability,
    Pdf,
    Github,
    Arxiv,
}

/// Provenance chain tracking where content came from.
///
/// Every piece of extracted content carries a provenance record that
/// describes how it was acquired. Chained `derived_from` allows
/// tracing multi-hop acquisition (e.g., search result → follow).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provenance {
    pub source_url: String,
    #[serde(default)]
    pub method: ExtractionMethod,
    pub agent: String,
    #[serde(default = "Utc::now")]
    pub accessed_at: DateTime<Utc>,
    pub duration_ms: u64,
    pub derived_from: Option<Box<Provenance>>,
}

impl Default for Provenance {
    fn default() -> Self {
        Self {
            source_url: String::new(),
            method: ExtractionMethod::default(),
            agent: String::new(),
            accessed_at: Utc::now(),
            duration_ms: 0,
            derived_from: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn test_provenance_serde_roundtrip() {
        let p = Provenance {
            source_url: "https://example.com".into(),
            method: ExtractionMethod::Follow,
            agent: "gthings/test".into(),
            accessed_at: Utc::now(),
            duration_ms: 100,
            derived_from: None,
        };
        let json = serde_json::to_string(&p).unwrap();
        let back: Provenance = serde_json::from_str(&json).unwrap();
        assert_eq!(p.source_url, back.source_url);
        assert_eq!(p.method, back.method, "method mismatch");
        assert_eq!(p.agent, back.agent);
        assert!(back.derived_from.is_none());
    }

    #[test]
    fn test_provenance_with_derived_from() {
        let parent = Provenance {
            source_url: "https://original.com".into(),
            method: ExtractionMethod::Search,
            agent: "gthings/search".into(),
            accessed_at: Utc::now(),
            duration_ms: 50,
            derived_from: None,
        };
        let child = Provenance {
            source_url: "https://example.com".into(),
            method: ExtractionMethod::Follow,
            agent: "gthings/follow".into(),
            accessed_at: Utc::now(),
            duration_ms: 200,
            derived_from: Some(Box::new(parent)),
        };
        let json = serde_json::to_string(&child).unwrap();
        let back: Provenance = serde_json::from_str(&json).unwrap();
        assert_eq!(back.source_url, "https://example.com");
        assert!(back.derived_from.is_some());
        let parent_back = back.derived_from.unwrap();
        assert_eq!(parent_back.source_url, "https://original.com");
        assert_eq!(
            parent_back.method,
            ExtractionMethod::Search,
            "method should be Search"
        );
    }

    #[test]
    fn test_provenance_default_access_time() {
        // Verify serde default for accessed_at works (should be Utc::now or similar)
        let json = r#"{"source_url":"https://x.com","method":"Follow","agent":"gthings/test","duration_ms":0}"#;
        let p: Provenance = serde_json::from_str(json).unwrap();
        assert_eq!(p.source_url, "https://x.com");
        assert_eq!(
            p.method,
            ExtractionMethod::Follow,
            "method should be Follow"
        );
        // accessed_at should be filled by default (not 1970)
        let age = Utc::now() - p.accessed_at;
        assert!(age.num_seconds() < 10); // Created within last 10 seconds
    }

    #[test]
    fn test_provenance_extraction_method_variants() {
        // Test all variants serialize/deserialize
        let methods = vec![
            ExtractionMethod::Follow,
            ExtractionMethod::Search,
            ExtractionMethod::Readability,
            ExtractionMethod::Pdf,
            ExtractionMethod::Github,
            ExtractionMethod::Arxiv,
        ];
        for m in methods {
            let json = serde_json::to_string(&m).unwrap();
            let back: ExtractionMethod = serde_json::from_str(&json).unwrap();
            assert_eq!(m, back, "ExtractionMethod roundtrip failed for {:?}", m);
        }
    }
}
