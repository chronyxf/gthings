# Changelog

## 0.3.8 (2026-08-04)

### Fixes

#### - refactor: remove DiskCache module (crates/common/src/cache.rs:1)

- chore: update doc comment to reference atomic_write (crates/common/src/domain_reputation.rs:5)
- refactor: remove cache module re-export from lib.rs (crates/common/src/lib.rs:1)

## 0.3.7 (2026-07-30)

### Fixes

#### - refactor: rename Sha256DiskCache → DiskCache, add SHA-256 key hashing (cache.rs)

- refactor: add in-memory domain reputation cache, extract helpers (domain_reputation.rs)
- chore: add hex and psl deps to Cargo.toml
- fix: use Self::Other in error.rs impls
- refactor: add safe_truncate_end, disconnect_arc, quality_flag_is_blocking (lib.rs)
- refactor: build_pagination returns Result, add EncodedToken struct (pagination.rs)
- refactor: use Box<Self>, Eq derives, add doc fixes (provenance.rs)
- refactor: add try_parse_url, strip_suffix, canonicalize_parsed_url helpers (url_normalizer.rs)
- refactor: use psl crate for registered_domain instead of hardcoded TLD list
- refactor: #[must_use] on all pure pub functions

## [0.3.6] - 2026-07-30

### Chore
- Remove dead cache_key function and unused imports

 — gthings-common

## 0.3.5 (2026-07-26)

### Changed

- Refactor cache.rs: atomic_write, base64 cache_key, is_file_expired
- Add url_normalizer, domain_reputation, pagination, provenance modules
- Add GTHINGS_AGENT, extract_host, is_file_expired, atomic_write to lib.rs
- Add chrono, base64, url, tempfile deps; remove tracing-subscriber

## 0.3.4 (2026-07-26)

### Removals

- Remove `trace` module (TraceWriter, TraceEvent, JSONL tracing)
- Remove SHA-256 key generation and `evict_expired` from Sha256DiskCache
- Cache callers now pass pre-computed keys externally

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
