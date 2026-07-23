# Changelog — common

## 0.1.0 — 2026-07-23

## 0.1.1 — 2026-07-23

### Fixes

- - Fix clippy warnings in extraction crate (collapsible_if, from_str_radix, map_or)

### Changed

- - Replace 23 unwrap() calls with expect() or proper error handling
- - Rename gthings-cdp crate to cdp for consistency
- - Remove decorative comment separators (═══, ───) across all files
- - Clean up outdated TypeScript references in comments


## 0.1.1 — 2026-07-23

### Features

- - Add new cdp crate: Browser launch, CDP connection, tab lifecycle
- - Add TraceWriter for step-level JSONL logging

### Changed

- - Remove legacy protocol/, cdp-core/, browser-daemon/ crates
- - Clean workspace Cargo.toml, .gitignore, .githooks


### Features
- Sha256DiskCache with persistent disk storage
- GthingsConfig with env var mapping
- TraceWriter for step-level JSONL tracing (timestamps, durations, URLs, errors)
- GthingsError type with Cdp variant
