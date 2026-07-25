# Changelog — gthings-search

## 0.3.6 (2026-07-25)

### Fixes

#### - Fix: update Google SERP CSS selectors from deprecated `div.g`, `div.yuRUbf` to current `div.tF2Cxc`, `div.MjjYud`

- Fix: update snippet selectors to include `.IsZvec`, `.GI74Re`, `.kb0PBd` in addition to existing `.VwiC3b`

## 0.3.5 (2026-07-25)

### Fixes

- Fix: update gthings-cdp dependency to 0.4.4 which corrects the profile path to include `/User Data` subdirectory and removes the crashing `--profile-directory` flag

## 0.3.4 (2026-07-25)

### Fixes

- Fix: update gthings-cdp dependency to 0.4.3 which reads `profile.last_used` from `Local State` and passes `--profile-directory=<last_used>` so the browser opens the real user profile (with bookmarks, history, cookies) instead of a fresh Default profile

## 0.3.3 (2026-07-25)

### Fixes

- Fix: update gthings-cdp dependency to 0.4.2 for real profile detection with SingletonLock-safe browser reuse

## 0.3.2 (2026-07-25)

### Fixes

- Fix: update gthings-cdp dependency to 0.4.1 which uses temporary browser profiles, eliminating "Something went wrong when opening your profile" errors during search operations

## 0.3.1 — 2026-07-24

### Changed
- Rename common→gthings-common, cdp→gthings-cdp, extraction→gthings-extraction, search→gthings-search for crates.io publishing
- Performance: spawn_blocking for sync I/O in browser.rs, struct field reordering for memory layout
- Performance: Vec<String>→QualityReason enum in quality scoring (eliminates 8 allocs/validate)
- Performance: Cow<str> fast path for URL normalization (0 allocs for 80% of calls)
- Performance: Derive Deserialize on SearchResult (3 fewer allocs per result)
- Chore: Remove stale/verbose comments across all crates (trimmed ~155 lines)
- Chore: Remove criterion benchmarks and revert Cargo.toml
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
