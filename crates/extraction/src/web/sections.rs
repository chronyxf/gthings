//! Section extraction and tree building from headings.
//!
//! [`WebExtractor::build_sections_from_headings`] maps extracted heading
//! tuples to offsets in the full text; [`WebExtractor::build_section_tree`]
//! nests a flat offset-sorted list using the HTML outline algorithm.

use crate::article::Section;

use super::WebExtractor;

/// Minimum heading text length (bytes) for a heading to be considered real.
pub(crate) const MIN_HEADING_LEN: usize = 3;
/// Maximum heading text length (bytes) before a "heading" is likely noise.
pub(crate) const MAX_HEADING_LEN: usize = 100;

impl WebExtractor {
    /// Build a nested section tree from a list of heading `(depth, text)`
    /// pairs extracted during the streaming pass, mapping each heading to
    /// its byte offset in `full_text`.
    pub(super) fn build_sections_from_headings(
        headings: &[(u8, String)],
        full_text: &str,
    ) -> Vec<Section> {
        if headings.is_empty() {
            return Vec::new();
        }

        // Drop implausibly short/long heading text (the same heuristic the
        // removed flat-text extractor used) so noise never becomes a section.
        let headings: Vec<&(u8, String)> = headings
            .iter()
            .filter(|(_, text)| {
                let len = text.len();
                (MIN_HEADING_LEN..=MAX_HEADING_LEN).contains(&len)
            })
            .collect();

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
            return Vec::new();
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
}

impl WebExtractor {
    /// Convert a flat list of offset-sorted sections into a nested tree
    /// using heading depth (HTML outline algorithm).
    pub(super) fn build_section_tree(flat: Vec<Section>) -> Vec<Section> {
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

    pub(super) fn insert_into_sections(sections: &mut Vec<Section>, section: Section, depth: u8) {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    // Section building from heading tuples

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
        // No headings → no sections (the flat-text heuristic is no longer
        // consulted; full text is whitespace-joined and has no headings).
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
}
