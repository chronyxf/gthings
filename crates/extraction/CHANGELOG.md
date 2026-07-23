# Changelog — extraction

## 0.3.0 — 2026-07-23

### Fixes

- from_str_radix with radix 10 → str::parse in HTML hex parsing
- map_or(false, ...) → is_some_and in uppercase detection
- Manual arithmetic → saturating_sub in PDF stream position
- Identical if branches merged in PDF decompression error handling
- Char range comparison → matches! in quality bot detection
- Regex::new().unwrap() → .expect("valid regex") across all modules
- scraper::Selector::parse("body").unwrap() → .expect("valid selector")
- trimmed.chars().last().unwrap() → .unwrap_or(' ')

### Changed

- Removed outdated "Ported from TypeScript" comments
- Cleaned decorative separator comments
