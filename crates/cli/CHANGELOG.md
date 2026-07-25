# Changelog — gthings (CLI)

## 0.4.1 (2026-07-25)

### Features

- batch harvest with parallel sessions and SERP filtering
- add daemon context to trace and browser type detection
- add direct CDP browser automation crate

### Fixes

- daemonize, close orphan tabs, add changeset tooling
- format changelog

## 0.4.0 (2026-07-25)

### Features

#### - Feat: simplify commands — remove `gthings init`, merge shell setup into `gthings update` as all-in-one command (binary update + shell PATH config + skill install)

- Feat: add shell detection utility (bash, zsh, fish) with auto-PATH configuration to shell config files
- Feat: add `gthings skill add --opencode/--agents/--all` command for standalone skill installation
- Chore: strip emoji from all CLI output
- Chore: update embedded skill documentation for simplified command structure

## 0.3.5 (2026-07-24)

### Features

#### - Feat: add `gthings update` command — runs `cargo install gthings`

- Feat: add `gthings skill add --opencode/--agents/--all` command — installs embedded skill files
- Refactor: consolidate from 6 opencode skills to 1, merge reference files into SKILL.md
- Test: 9 unit tests for embedded skill structure, 5 integration tests for skill install
- Chore: remove old scripts/install-skills.sh, delete merged reference files (errors/agent-trace/agent-prompt)

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
- Persistent browser auto-launch on first command, reuse across invocations
- browser start, stop, status commands for explicit lifecycle management
- --trace flag for step-level JSONL telemetry (browser, tab, navigate, extract events)
- All commands: search query, search batch, search harvest, follow url, follow batch, pdf url, pdf file

### Changed
- Removed daemon-based browser management (browser-daemon crate deleted)
- Simplified from 3-process architecture (CLI → daemon → Chrome) to single process + persistent Chrome
- Command handlers now use direct CDP via cdp crate instead of UDS daemon RPC
- Function visibility corrected to pub(crate) for handler functions

### Removed
- screenshot and scrape subcommands (legacy, not yet reimplemented for new architecture)
