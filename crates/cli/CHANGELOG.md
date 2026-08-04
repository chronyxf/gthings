# Changelog

## 0.6.8 (2026-08-04)

### Features

#### - feat: add --engine flag and multi-engine search wiring (main.rs:98)

- feat: add machine-parseable describe subcommand (main.rs:405)
- feat: open CDP only when a browser engine is needed (search.rs:36)
- feat: emit per-query result/error entries in batch output (batch.rs:49)
- chore: bump cdp/search/extraction dependency versions (Cargo.toml:10)
- chore: raise default max_chars to 40000 and search timeout to 60s (main.rs:189)
- chore: update agent skill docs for envelope/engines/operators (SKILL.md:34)
- chore: add CLI tests for max-chars and describe (cli_tests.rs:211)

## 0.6.7 (2026-07-30)

### Fixes

#### - fix: parallel strategy output wrapped in {"results": ...} (batch.rs)

- refactor: extract with_session() lifecycle helper (helpers.rs, batch.rs, harvest.rs)
- refactor: extract dispatch functions from main() (main.rs)
- refactor: remove dead code in update.rs
- fix: replace expect() with unwrap_or_else in helpers.rs
- fix: remove unused async from handle_pdf_extraction (pdf.rs)
- fix: remove unnecessary .to_vec() on Bytes (pdf.rs)
- fix: inline port() function (connect.rs, status.rs)
- fix: clippy lints — uninlined_format_args, single_match_else (main.rs, connect.rs, update.rs, harvest.rs)
- refactor: elide lifetimes, reduce Vec allocation in text_summary (helpers.rs)

## [0.6.6] - 2026-07-30

### Feat
- Add UniversalFlags struct shared across all commands: --output, --query, --cdp-port, --timeout, -v, -q
- Add gthings ax <url> command for accessibility tree extraction
- Add gthings pdf-url and pdf-file commands
- Add --strategy simple|parallel|harvest to search, replacing separate batch/harvest subcommands
- Add 16 integration tests for new flags and ax command
- Update README.md and all 4 skill files to match new CLI

### Fix
- Fix empty query panic (guard before queries[0] access)
- Fix CDP port config silently ignored (merge_from propagates flags)
- Fix --output text showing JSON instead of human-readable text
- Fix e2e test and CLI test assertions for new output format


## [0.6.5] - 2026-07-29

### Fixed
- Correct agent skill install path from `~/.agents/gthings/` to `~/.agents/skills/gthings/` to match opencode spec (update.rs)

## [0.6.4] - 2026-07-26

### Fixed
- Agent skill files now install to `~/.agents/gthings/` (was `~/.config/opencode/agents/gthings/`)

## [0.6.3] - 2026-07-26

### Added
- Add `gthings update` CLI command that updates the binary via `cargo install gthings` and installs/refreshes opencode skill files to `~/.config/opencode/skills/gthings/` and agent files to `~/.config/opencode/agents/gthings/`

## 0.6.1 (2026-07-26)

### Added
- New harvest subcommand: full research pipeline with --follow-top, --dedup, --rank
- Updated skill files for AI agent consumption (agents + opencode)

### Changed
- Fix import ordering (crate before external) across all command files
- Fix enum variant paths in harvest.rs to use short form
- Replace ⚠ emoji with [!] in warning output strings
- Fix silent JSON serialization failure handling in extract.rs

## 0.6.0 (2026-07-26)

### Breaking Changes

- Replace shell-based REPL with flat CLI subcommands
- Remove shell.rs (400 lines of shell detection/PATH management)
- Add commands/ module with 6 subcommand handlers (batch, connect, extract, follow, search, status)
- Simplify main.rs from 350 lines to ~100-line dispatcher
- Add `extract` command for auto-detected URL extraction (PDF/GitHub/arXiv/web)
- Add `pdf` subcommand with url/file targets

## 0.5.0 (2026-07-25)

### Breaking Changes

#### - Simplified CLI from 7+ nested subcommands to 4 flat subcommands: search, follow, batch, status

- Removed PDF extraction commands (pdf url, pdf file)
- Removed browser lifecycle commands (browser start, browser stop) — browser is detected-only now
- Removed update command (cargo install + shell config + skill installation)
- Removed skill management commands
- Removed --trace flag and telemetry infrastructure
- Removed embedded skills directory (include_dir!)
- Removed follow_commands, search_commands, pdf_commands modules
- Changed --max to --max-chars for follow and batch subcommands
- --json is now per-subcommand flag (not global)
- Added structured error JSON output (print_error helper)

## 0.4.9 (2026-07-25)

### Fixes

#### - Fix: tab creation for Dia — use `Target.attachToTarget` when `createTarget` returns no `sessionId` instead of failing via HTTP

- Fix: `find_existing()` now discovers WS URL from DevToolsActivePort across all common browser profile paths (Dia, Chrome, Brave, Edge)
- Fix: `handle_browser_status()` no longer falls back to `/tmp` for profile detection
- Add: `dismiss_allow_debugging_dialog()` — macOS AppleScript to auto-click "Allow" on "Allow debugging connection?" dialog
- Remove: `BrowserState`, `state_path()`, `pid()`, `home_dir()` — fully stateless
- Remove: `browser_state_path()` from CLI — no more `~/.gthings/browser.json`
- Test: 7 integration tests for DevToolsActivePort parsing, WS URL discovery, port probing
- Test: 4 E2E tests for stateless detection, no state file, dialog dismiss, content extraction

## 0.4.8 (2026-07-25)

### Fixes

#### - Fix: `browser stop` now uses `lsof -ti:<port>` to find browser PIDs instead of state file

- Fix: `browser status` no longer requires state file — probes CDP port directly via DevToolsActivePort
- Remove: `browser_state_path()` function — no more state file dependency

## 0.4.7 (2026-07-25)

### Fixes

#### - Refactor: remove browser state file (`~/.gthings/browser.json`) — fully stateless browser management

- Fix: `browser stop` now uses `lsof -ti:<port>` to find and kill browser PIDs instead of reading state file
- Fix: `browser status` no longer requires state file — probes CDP port directly

## 0.4.6 (2026-07-25)

### Fixes

- Fix: update gthings-cdp dependency to 0.4.4 which corrects the profile path to include `/User Data` subdirectory and removes the crashing `--profile-directory` flag

## 0.4.5 (2026-07-25)

### Fixes

- Fix: update gthings-cdp dependency to 0.4.3 which reads `profile.last_used` from `Local State` and passes `--profile-directory=<last_used>` so the browser opens the real user profile (with bookmarks, history, cookies) instead of a fresh Default profile

## 0.4.4 (2026-07-25)

### Fixes

- Fix: update gthings-cdp dependency to 0.4.2 for real profile detection with SingletonLock-safe browser reuse

## 0.4.3 (2026-07-25)

### Fixes

- Fix: update gthings-search dependency to 0.3.2 which uses temp browser profiles for search operations

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


