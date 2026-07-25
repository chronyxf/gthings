# Changelog — gthings-common

## 0.3.3 (2026-07-25)

### Fixes

#### - Removed config module (GthingsConfig, from_env) — configuration now uses ad-hoc env var reads in CLI

- Removed logging module (init_tracing, TraceWriter) — tracing setup moved to CLI
- Simplified error module — removed unused error variants
- Simplified lib.rs exports to only cache, error, trace modules

## 0.3.2 — 2026-07-24

### Changed
- Rename common→gthings-common, cdp→gthings-cdp, extraction→gthings-extraction, search→gthings-search for crates.io publishing
- Performance: spawn_blocking for sync I/O in browser.rs, struct field reordering for memory layout
- Performance: Vec<String>→QualityReason enum in quality scoring (eliminates 8 allocs/validate)
- Performance: Cow<str> fast path for URL normalization (0 allocs for 80% of calls)
- Performance: Derive Deserialize on SearchResult (3 fewer allocs per result)
- Chore: Remove stale/verbose comments across all crates (trimmed ~155 lines)
- Chore: Remove criterion benchmarks and revert Cargo.toml

### Fixes
- Fix: Add missing .await on browser.pid() calls in cli/src/main.rs

## 0.3.1 — 2026-07-23

### Changed
- Remove SKIP_CHECKS bypass from pre-commit hook
- Run e2e tests serially (--test-threads=1) to avoid port conflicts
- Run fmt, clippy, build, unit, integration, e2e checks on every code commit

### Features
- TraceWriter: step-level JSONL tracing with timestamps, durations, URLs, errors
- CdpError variant added to GthingsError for CDP transport errors

### Removed
- Removed framing.rs (UDS length-prefix framing — daemon no longer exists)
- Removed rate_limit.rs (per-host token bucket — daemon no longer exists)
- Removed dead_code allowances from cached types
- Cleaned up TypeScript-origin comments from cache module

### Fixes
- Collapsible if in cache eviction logic
