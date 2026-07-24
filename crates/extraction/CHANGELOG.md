# Changelog — extraction

## 0.3.0 — 2026-07-23

## 0.3.1 — 2026-07-24

### Changed

- Performance: spawn_blocking for sync I/O in browser.rs, struct field reordering for memory layout
- Performance: Vec<String>→QualityReason enum in quality scoring (eliminates 8 allocs/validate)
- Performance: Cow<str> fast path for URL normalization (0 allocs for 80% of calls)
- Performance: Derive Deserialize on SearchResult (3 fewer allocs per result)
- Chore: Remove stale/verbose comments across all crates (trimmed ~155 lines)
- Chore: Remove criterion benchmarks and revert Cargo.toml


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
