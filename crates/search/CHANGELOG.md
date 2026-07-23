# Changelog — search

## 0.1.0 — 2026-07-23

## 0.1.1 — 2026-07-23

### Fixes

- - Fix clippy warnings in extraction crate (collapsible_if, from_str_radix, map_or)

### Changed

- - Replace 23 unwrap() calls with expect() or proper error handling
- - Rename gthings-cdp crate to cdp for consistency
- - Remove decorative comment separators (═══, ───) across all files
- - Clean up outdated TypeScript references in comments


## 0.2.0 — 2026-07-23

### Features

- - Add filter_deny_hosts and rank_results helpers

### Changed

- - Replace UDS daemon RPC with direct CDP calls via cdp crate
- - Remove daemon RPC types (BatchSearchRequest, BatchFollowRequest, HarvestRequest)
- - Use persistent browser with tab create/close per operation


### Features
- Google search with organic result extraction via CDP Runtime.evaluate
- Page follow with content extraction and quality gate
- Batch search, batch follow, and bulk harvest pipeline
- filter_deny_hosts for result filtering
- Result ranking by tiebreaker
- arXiv URL normalization

### Changed
- Removed daemon RPC types (BatchSearchRequest, BatchFollowRequest, HarvestRequest)
- Changed from UDS daemon calls to direct CDP via gthings-cdp crate
