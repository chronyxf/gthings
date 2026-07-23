# Changelog — common

## 0.3.0 — 2026-07-23

## 0.3.1 — 2026-07-23

### Changed

- - Remove SKIP_CHECKS bypass from pre-commit hook
- - Run e2e tests serially (--test-threads=1) to avoid port conflicts
- - Run fmt, clippy, build, unit, integration, e2e checks on every code commit


### Features

- TraceWriter: step-level JSONL tracing with timestamps, durations, URLs, errors
- CdpError variant added to GthingsError for CDP transport errors

### Changed

- Removed framing.rs (UDS length-prefix framing — daemon no longer exists)
- Removed rate_limit.rs (per-host token bucket — daemon no longer exists)
- Removed dead_code allowances from cached types
- Cleaned up TypeScript-origin comments from cache module

### Fixes

- Collapsible if in cache eviction logic
