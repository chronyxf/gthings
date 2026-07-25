# Changelog — gthings-cdp

## 0.4.1 (2026-07-25)

### Fixes

- Fix: switch to temporary profile by default to avoid "Something went wrong when opening your profile" error. Removes real_profile_dir() / browser_profile_suffix() functions that tried to match browser to real user profile (which caused SingletonLock conflicts when the browser was already running).
- Chore: add CDP launch stability flags (`--enable-automation`, `--disable-background-networking`, `--disable-extensions`, `--disable-component-update`, `--disable-default-apps`, `--password-store=basic`, `--use-mock-keychain`)

## 0.4.0 (2026-07-25)

### Features

- Feat: macOS default browser detection — uses Launch Services to find the user's default Chromium browser (Chrome, Dia, Arc, Brave, Edge)
- Feat: browser-profile matching — automatically selects the correct profile directory for the detected browser, preventing profile format mismatch errors
- Feat: config env vars (GTHINGS_BROWSER_PATH, GTHINGS_PROFILE_DIR, GTHINGS_CDP_PORT) now wired into browser launch
- Feat: fallback to temp profile `/tmp/gthings-{port}` when no real profile is found
- Chore: added `--no-default-browser-check` and `--disable-sync` flags for launch stability

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
- Direct CDP browser automation crate (Browser, Connection, Tab)
- Persistent browser mode: launch once, reuse across commands (port 9222)
- CDP WebSocket oneshot command dispatch with pending response tracking
- Tab lifecycle: create via CDP Target.createTarget or HTTP /json/new fallback, navigate, extract, close
- Real Dia/Chrome profile auto-detection with lock file cleanup
- Content quality gate (score, is_ok, reasons) integrated into extraction
- TraceWriter for step-level debugging (browser_reuse, tab_create, navigate, extract, tab_close)

### Fixes
- IPv4 + IPv6 dual-stack port probing for Dia browser
- Atomic state file write (temp file + rename) to prevent race conditions
- Tab close: window.close() then Target.closeTarget with 200ms delay
- Port 9222 TIME_WAIT handled via retry loop

### Changed
- Complete rewrite from per-command Chrome launch to persistent browser model
- Dynamic serde_json::Value types replace generated CDP protocol (removed 24K LOC generated code)
