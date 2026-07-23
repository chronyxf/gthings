# Changelog — extraction

## 0.1.0 — 2026-07-23

## 0.1.1 — 2026-07-23

### Fixes

- - Fix clippy warnings in extraction crate (collapsible_if, from_str_radix, map_or)

### Changed

- - Replace 23 unwrap() calls with expect() or proper error handling
- - Rename gthings-cdp crate to cdp for consistency
- - Remove decorative comment separators (═══, ───) across all files
- - Clean up outdated TypeScript references in comments


### Features
- HTML extraction with CSS selector support
- Section detection from heading tags
- PDF text extraction (flate2 decompression)
- Content quality gate: captcha, bot, paywall, empty shell detection
- Content truncation with offset/max pagination
- arXiv URL auto-detection for PDF extraction
