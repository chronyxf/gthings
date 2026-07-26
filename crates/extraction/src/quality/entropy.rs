use std::collections::HashMap;

/// Compute the character-level Shannon entropy of a string in bits per character.
///
/// H = - Σ p(c) · log₂(p(c))
///
/// where p(c) is the relative frequency of each Unicode character in `text`.
///
/// Returns 0.0 for empty or whitespace-only strings.
pub fn shannon_entropy(text: &str) -> f32 {
    let text = text.trim();
    if text.is_empty() {
        return 0.0;
    }

    // Single pass: count character frequencies
    let mut freq: HashMap<char, usize> = HashMap::new();
    for c in text.chars() {
        *freq.entry(c).or_insert(0) += 1;
    }

    let total = text.len() as f32;
    let mut entropy = 0.0_f32;

    for &count in freq.values() {
        if count == 0 {
            continue;
        }
        let p = count as f32 / total;
        entropy -= p * p.log2();
    }

    entropy
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_entropy_empty() {
        assert_eq!(shannon_entropy(""), 0.0);
        assert_eq!(shannon_entropy("   "), 0.0);
    }

    #[test]
    fn test_entropy_repeated_char() {
        let h = shannon_entropy("aaaaaaaaaa");
        assert!(
            h < 0.01,
            "entropy of repeated char should be near 0, got {h}"
        );
    }

    #[test]
    fn test_entropy_two_chars_equal() {
        // 5 'a's + 5 'b's → p = 0.5 each → H = 1.0 bit/char
        let h = shannon_entropy("aaaaabbbbb");
        let diff = (h - 1.0).abs();
        assert!(
            diff < 0.01,
            "entropy of 2-symbol uniform should be 1.0, got {h}"
        );
    }

    #[test]
    fn test_entropy_uniform_alphabet() {
        // 4 distinct chars, equal freq → H = log2(4) = 2.0
        let s: String = (0..100).map(|i| char::from(b'a' + (i % 4))).collect();
        let h = shannon_entropy(&s);
        let diff = (h - 2.0).abs();
        assert!(
            diff < 0.05,
            "entropy of 4-symbol uniform should be ~2.0, got {h}"
        );
    }

    #[test]
    fn test_entropy_english_paragraph() {
        let paragraph = "The quick brown fox jumps over the lazy dog. This classic pangram contains every letter of the English alphabet at least once. It has been used for typing practice and font display for decades.";
        let h = shannon_entropy(paragraph);
        assert!(
            (3.0..=5.5).contains(&h),
            "English paragraph entropy should be in 3.0-5.5 range, got {h}"
        );
    }

    #[test]
    fn test_entropy_single_char() {
        let h = shannon_entropy("x");
        assert!(h < 0.01, "single char entropy should be 0, got {h}");
    }

    #[test]
    fn test_entropy_high_entropy() {
        // Many distinct chars, roughly uniform → high entropy
        let s: String = (0..=255).map(|i| char::from(b' ' + (i % 95))).collect();
        let h = shannon_entropy(&s);
        // max entropy for 95 printable ASCII chars ≈ log2(95) ≈ 6.57
        assert!(
            h > 5.5,
            "highly varied text should have entropy > 5.5, got {h}"
        );
    }
}
