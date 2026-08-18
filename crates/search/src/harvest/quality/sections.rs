//! Section-like structure extraction from plain text content.

use gthings_extraction::article::Section;

/// Extract section-like structure from plain text content.
///
/// Uses double-newline block splitting: if a block's first line is a short
/// line that doesn't end with sentence punctuation, it's treated as a heading.
///
/// Supports two formats:
/// - **Format A** — Heading and content in the same `\n\n` block, separated
///   by a single newline: `"Heading\nContent line 1\nContent line 2"`.
/// - **Format B** — Heading and content in separate `\n\n` blocks:
///   `"Heading\n\nContent paragraph"`.
///
/// `pub(crate)` because the re-export in the parent `quality` module re-exposes
/// it at `pub(super)` (harvest) scope; the public surface is unchanged.
pub(crate) fn extract_sections(content: &str) -> Vec<Section> {
    if content.len() < 50 {
        return Vec::new();
    }

    let mut sections = Vec::new();
    let blocks: Vec<&str> = content.split("\n\n").collect();
    let mut offset = 0;
    let mut i = 0;

    // Returns `true` if `line` looks like a section heading.
    let is_heading = |s: &str| -> bool {
        let t = s.trim();
        !t.is_empty()
            && t.len() < 100
            && !t.ends_with('.')
            && !t.ends_with('!')
            && !t.ends_with('?')
            && t.chars().filter(|&c| c == ' ').count() < 12
    };

    while i < blocks.len() {
        let raw = blocks[i];
        let block = raw.trim();
        let block_start = offset;
        offset += raw.len() + 2;

        if block.is_empty() {
            i += 1;
            continue;
        }

        let lines: Vec<&str> = block.lines().collect();

        // Format A: multi-line block with heading as first line
        if lines.len() >= 2 && is_heading(lines[0]) {
            sections.push(Section {
                heading: lines[0].trim().to_string(),
                depth: 2,
                offset: block_start,
                length: raw.len(),
                content: lines[1..].join("\n"),
                subsections: Vec::new(),
            });
            i += 1;
            continue;
        }

        // Format B: single-line heading followed by content in next block
        if lines.len() == 1 && is_heading(block) && i + 1 < blocks.len() {
            let next_raw = blocks[i + 1];
            let next_block = next_raw.trim();
            if !next_block.is_empty() {
                let next_lines: Vec<&str> = next_block.lines().collect();
                let next_is_heading =
                    next_lines.len() == 1 && next_block.len() < 100 && is_heading(next_block);

                if !next_is_heading {
                    sections.push(Section {
                        heading: block.to_string(),
                        depth: 2,
                        offset: block_start,
                        length: raw.len() + 2 + next_raw.len(),
                        content: next_block.to_string(),
                        subsections: Vec::new(),
                    });
                    // Skip the content block too
                    offset += next_raw.len() + 2;
                    i += 2;
                    continue;
                }
            }
        }

        i += 1;
    }

    sections
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Section extraction tests ─────────────────────────────────────────

    #[test]
    fn test_extract_sections_empty() {
        let sections = extract_sections("");
        assert!(sections.is_empty());
    }

    #[test]
    fn test_extract_sections_finds_headings() {
        let text = "Introduction\n\nHere is some introductory content.\n\n\
                     Background\n\nThis section provides background information.\n\n\
                     Conclusion\n\nThe final section wraps up.";
        let sections = extract_sections(text);
        assert!(!sections.is_empty());
        let headings: Vec<&str> = sections.iter().map(|s| s.heading.as_str()).collect();
        assert!(headings.contains(&"Introduction"));
        assert!(headings.contains(&"Background"));
    }
}
