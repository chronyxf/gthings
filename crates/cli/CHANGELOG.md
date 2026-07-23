# Changelog — gthings (CLI)

## 0.1.0 — 2026-07-23

## 0.1.1 — 2026-07-23

### Changed

- - Rewrite README.md with current architecture, commands, benchmark
- - Rewrite VERSION.md for per-crate changelog workflow
- - Update consume-changesets.sh to write per-crate CHANGELOG.md files
- - Update SKILL.md and reference docs for AI agent usage
- - Remove daemon.md (daemon no longer exists)


## 0.1.1 — 2026-07-23

### Features

- - Add unit tests: cdp crate (10 tests), trace writer (3 tests), search helpers (4 tests)
- - Add CLI integration test (binary exists)
- - Add port wait helpers for sequential browser lifecycle tests

### Changed

- - Flatten tests/ to standard Rust structure (no #[path] hacks)
- - Rewrite e2e tests for direct CDP architecture (5 tests, 0 ignored)


## 0.1.1 — 2026-07-23

### Fixes

- - Fix clippy warnings in extraction crate (collapsible_if, from_str_radix, map_or)

### Changed

- - Replace 23 unwrap() calls with expect() or proper error handling
- - Rename gthings-cdp crate to cdp for consistency
- - Remove decorative comment separators (═══, ───) across all files
- - Clean up outdated TypeScript references in comments


## 0.2.0 — 2026-07-23

### Fixes

- - Fix function visibility (pub(crate) for handler functions)

### Changed

- - Remove daemon browser commands (start/stop/status simplified)
- - Wire --trace flag through all command handlers
- - Integrate persistent browser for all search/follow/pdf operations


### Features
- search query, search batch, search harvest commands
- follow url, follow batch commands
- pdf url, pdf file commands
- browser start, stop, status commands
- --json flag for structured output
- --trace flag for step-level JSONL telemetry
- Persistent browser (auto-launch on first command, reuse across commands)

### Changed
- Removed daemon-based browser management
- Simplified to direct CDP via persistent browser on port 9222
