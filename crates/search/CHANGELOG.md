# Changelog — search

## 0.3.0 — 2026-07-23

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
