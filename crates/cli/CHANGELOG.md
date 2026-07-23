# Changelog — gthings (CLI)

## 0.3.0 — 2026-07-23

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
