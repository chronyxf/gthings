# Changelog — search

## 0.3.0 — 2026-07-23

## 0.3.1 — 2026-07-24

### Changed

- Performance: spawn_blocking for sync I/O in browser.rs, struct field reordering for memory layout
- Performance: Vec<String>→QualityReason enum in quality scoring (eliminates 8 allocs/validate)
- Performance: Cow<str> fast path for URL normalization (0 allocs for 80% of calls)
- Performance: Derive Deserialize on SearchResult (3 fewer allocs per result)
- Chore: Remove stale/verbose comments across all crates (trimmed ~155 lines)
- Chore: Remove criterion benchmarks and revert Cargo.toml


### Changed

- Replaced UDS daemon RPC (send_request via UnixStream + common::framing) with direct CDP via cdp crate
- Each operation (search, follow, batch) launches persistent browser or reuses existing connection
- Tab create/close per operation: create via HTTP /json/new, navigate, extract, close

### Removed

- Daemon RPC types: BatchSearchRequest, BatchFollowRequest, HarvestRequest, SearchHit, BatchSearchResponse, BatchFollowResponse, HarvestResponse
- UnixStream and framing dependencies from all three modules

### Features

- filter_deny_hosts helper for result filtering
- rank_results helper for result stability
- Sort_by_key replaces sort_by in batch ranking (clippy fix)

### Fixes

- removed #[allow(dead_code)] from unused fields
- replaced wildcard pub use types::* with explicit re-exports
- cleaned daemon-tainted doc comments
