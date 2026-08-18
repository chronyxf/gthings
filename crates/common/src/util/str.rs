/// Strip a suffix from the end of a string and trim the remainder.
///
/// Returns the remainder of the string (trimmed) if the suffix is present,
/// or the original string unchanged if the suffix is not found.
///
/// Unlike byte-level slicing (`s[..s.len()-n]`), this avoids panicking on
/// multi-byte characters such as non-breaking space (U+00A0), CJK, or emoji.
#[must_use]
pub fn strip_suffix_and_trim(s: &str, suffix: &str) -> String {
    s.strip_suffix(suffix)
        .map_or_else(|| s.to_string(), |trimmed| trimmed.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::strip_suffix_and_trim;

    #[test]
    fn strips_known_suffix_and_trims() {
        assert_eq!(
            strip_suffix_and_trim(" hello world - ", " - "),
            "hello world"
        );
    }

    #[test]
    fn leaves_unknown_suffix_unchanged() {
        assert_eq!(strip_suffix_and_trim("hello world", "zzz"), "hello world");
    }

    #[test]
    fn respects_multibyte_boundaries() {
        assert_eq!(strip_suffix_and_trim("café…", "…"), "café");
    }
}
