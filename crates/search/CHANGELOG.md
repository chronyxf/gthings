# Changelog

## 0.7.3 (2026-07-30)

### Fixes

#### - fix: scroll iterations count.max(3) for better count delivery (search.rs)

- fix: collect count*2 results in JS template (search_extract.js)
- fix: only drop truly empty snippets not <5 chars (search.rs)
- fix: request count*2 from Google via num param (search.rs)
- fix: remove #[allow(incompatible_msrv)] vestigial annotations
- refactor: extract search_with_tab() helper (follow.rs)
- refactor: extract make_error_result() helper (follow.rs)
- refactor: extract classify_body_status(), map_join_err() (orchestrator.rs)
- refactor: replace BTreeMap with IndexMap for insertion order (ranking.rs)
- refactor: replace unconditional clone with scoped clone (search.rs)
- refactor: add tracing::warn on extract_host failure (batch.rs, follow.rs)
- refactor: use .pointer() for CDP result access (follow.rs)
- chore: add indexmap dep to Cargo.toml
- refactor: unify selection passes in harvest orchestrator
- chore: remove 2039-line harvest.rs merge artifact

## [0.7.2] - 2026-07-30

### Feat
- Add scroll-triggered lazy loading for more organic results
- Add method-aware quality scoring (skip length checks for PDF/Arxiv)
- Add result post-processing: junk URL filter, base URL dedup, title/snippet cleanup
- Add UTF-8 safe safe_truncate_end() helper

### Fix
- Fix UTF-8 char boundary panic in snippet cleaning (byte slicing -> strip_suffix)
- Fix domain_authority rounding to 2 decimal places


## [0.7.1] - 2026-07-29

### Added
- CAPTCHA/Sorry page detection after Google navigation — checks URL for `/sorry/` and page title for "Accessibility"/"Learn more", returns `CdpError::CaptchaBlocked` instead of fake results (search.rs)

## [0.7.0] - 

### Added
- Async SPA rendering polling with await and setTimeout (non-blocking) — JS frameworks can render content while extraction waits
- Compound content selectors: querySelector('main, article, [role="main"]') with fallback to body
- Conditional chrome element stripping: nav/footer/header removed only when a semantic container is found
- textContent fallback when innerText returns fewer than 80 characters (catches CSS-hidden SPA content)
- try/catch URL parsing and title length validation in search SERP extraction
- Additional search result selector fallbacks for varied Google SERP layouts
- 11 new unit tests for FollowResult/ SearchResult JSON parsing and extraction JS format string validation

### Fixed
- Replaced unused _query variable with bare _ in harvest ranking loop
- Robust JSON parse error handling with descriptive warnings in follow and search extraction

## 0.6.0 (2026-07-26)

### Added
- New harvest pipeline: search→dedup→rank→follow→quality in one command
- BodyStatus enum: ok, pdf_unextracted, extract_failed, chrome_or_empty, snippet_only
- HarvestRunSummary with coverage_by_query and warnings for agent triage
- select_follow_candidates with per-query minimum, per-host cap, junk URL filter
- quality reasons always non-empty when score low (bot_blocked, paywall, captcha, etc.)
- Dedup_key URL normalization with fragment/tracking-param stripping
- Direct tests for normalize_url, is_junk_url, composite rank correctness
- Follow quality flag detection tests (bot/paywall/captcha/empty)

## 0.5.0 (2026-07-25)

### Breaking Changes

#### - Removed shared types.rs module — SearchResult moved to search.rs, FollowResult moved to follow.rs

- Simplified SearchResult and FollowResult — removed unused fields (query, total_length, sections, quality, etc.)
- Removed two-phase harvest pipeline (search + follow) — BatchProcessor::harvest deleted
- Removed BatchProcessor::follow — multi-URL follow deleted
- Fixed batch timeout safety — close_tab now runs outside tokio::time::timeout, preventing window leaks on cancellation
- BatchProcessor::search now takes Arc<Session> instead of &self with GthingsConfig
- search and follow functions now accept &Session and &Tab instead of &mut Connection

## 0.3.8 (2026-07-25)

### Fixes

#### - Fix: refactor profile resolution to match gsearch's proven approach

- Add: `resolve_profile()` — prefers real profile if not in use, falls back to seeded temp profile
- Add: `seed_profile()` — writes synthetic Preferences and Local State with `distribution.skip_first_run_ui: true` to suppress ALL first-run dialogs including Dia's Atlassian sign-in form
- Fix: `launch()` only cleans locks on real profiles — temp profiles skip lock cleaning entirely
- Fix: `is_profile_in_use()` prevents launching with a profile that has a running browser, avoiding crashes
- Remove: `real_profile_dir()` — replaced by `resolve_profile()` with proper in-use detection
- Remove: overlay dismissal JS from search/batch/follow extraction (unnecessary with proper profile seeding)

## 0.3.7 (2026-07-25)

### Fixes

#### - Fix: dismiss Dia sign-in overlay and other blocking dialogs via CDP JavaScript before page content extraction

- Add: overlay removal JS (onboarding, signin, login, aria-modal dialogs, Atlassian iframes) to search, batch, and follow extraction

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

