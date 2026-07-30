# Changelog

## 0.5.3 (2026-07-30)

### Fixes

#### - refactor: replace expect() with if let Some in insert_into_sections (web.rs)

- refactor: replace map_err().ok() with if let Ok in jsonld.rs
- refactor: replace eprintln!() with tracing::warn!() in jsonld.rs, pdf.rs
- refactor: extract READ_MORE_INDICATOR const (quality/types.rs, validate.rs)
- fix: remove unnecessary params.clone() in dispatch.rs
- fix: simplify split().next().unwrap_or() to .next().unwrap() in dispatch.rs
- chore: add tracing dep to Cargo.toml
- refactor: decompose article.rs, extractor.rs, detection.rs, pdf.rs, web.rs

## [0.5.2] - 2026-07-30

### Feat
- Add ExtractionError::RateLimited variant with retry_after parsing
- Rewrite extract_github() with intelligent URL routing for blob/tree/diff/patch/repo-root
- Add semanticscholar.org and cell.com to high-authority domains
- Add HTTP 429 detection in PDF fetch and web extraction

### Fix
- Fix heading depth detection in WebExtractor (remove dead current_depth variable)
- Make extract_metadata an associated function (remove unused &self)


## [0.5.1] - 

### Fixed
- Replaced unused _root_num variable with bare _ in PDF metadata extraction

## [0.5.0] - 2026-07-26

### Added

- New entropy module (shannon_entropy character-level information density)
- DOM section tree: extract_sections_from_html with nested heading hierarchy
- Pagination support in web extraction (offset, max_chars, continuation tokens)
- SourceType dispatch fix in web.rs (removed unused import, clean routing)
- Extra quality heuristics (entropy-based ThinContent/Garbled flag detection)
- QualityFlag re-export from gthings_common::domain_reputation
- Provenance tracking for web extractions (method, agent, timestamp)
- entropy_bits_per_char and flags fields on QualityResult

### Changed

- Replace readability::extractor::scrape (double fetch) with extract on already-fetched HTML
- Unicode-safe text slicing (char boundary check instead of raw byte slicing)
- Emoji/CJK character handling in quality validation
- Enhanced test coverage for CJK, emoji, boundary-length, and short-threshold cases

### Removed

- Remove dead code: needs_recrawl, secondary_check, SecondaryResult struct
- Remove unused selectors and stale allow(dead_code) attributes

## [0.4.1] - 2026-07-26

### Fixed
- Replace pdftotext CLI (poppler-utils) with pdf-extract (bundled MuPDF) for PDF text extraction
- Fix TeX font extraction failure (Computer Modern fonts in arxiv papers)

## 0.4.0 (2026-07-26)

### Breaking Changes

- Replace pure-Rust PDF parser with pdftotext backend for robust extraction
- Remove 4,000-line custom PDF parser (content/, font/, parser/, text.rs)
- Add pdftotext-based PdfExtractor with quality scoring
- New modules: article.rs, dispatch.rs, extractor.rs, jsonld.rs, web.rs
- Restructure quality/ into modular submodules (detection, validate, types)
- Remove monolithic html.rs, quality.rs, old pdf.rs
- Add Extractor trait with AutoExtractor URL-type dispatch
- Add JSON-LD metadata extraction for web pages

## 0.3.2 (2026-07-25)

### Fixes

#### - Updated ContentQuality validation thresholds

- Internal refactoring in quality module

## 0.3.1 — 2026-07-24

### Changed
- Rename common→gthings-common, cdp→gthings-cdp, extraction→gthings-extraction, search→gthings-search for crates.io publishing
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

