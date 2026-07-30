# Changelog

## [0.5.3] - 2026-07-30

### Fix
- Reduce osascript dialog dismissal timeout (5s to 1s, 2 attempts to 1)
- Try WebSocket connection first, skip osascript when dialog not present
- Reduce per-command startup overhead from ~1.6s to ~0s in common case


## [0.5.2] - 2026-07-30

### Feat
- Add ax_tree module: ax_tree(), ax_query(), compress_ax_tree(), ax_diff() with LCS diff
- Add auto-reconnect with exponential backoff and WS URL detection cascade
- Add with_isolated_tab() and run_in_tab() for per-operation tab isolation
- Add create_background_tab() with CDP background flag
- Convert dialog dismissal to async tokio; reduce MAX_ATTEMPTS 5->2
- Add Connection::call() retry on ConnectionFailed
- Add Drop impls for Connection and Session

### Fix
- Replace lock().unwrap() with lock().expect() for panic safety
- Remove dead reconnect constants and reconnect_and_retry method


## [0.5.1] - 2026-07-29

### Fixed
- Replace unbounded osascript blocking call with mpsc channel + 5s recv_timeout, preventing macOS System Events hang (browser.rs)
- Add session_id filter to networkIdle lifecycle event predicate — prevents cross-tab event matching with many open tabs (session.rs)
- Add stealth JS injection via `Page.addScriptToEvaluateOnNewDocument` (navigator.webdriver + languages override) and `Network.setUserAgentOverride` to avoid Google CDP detection (session.rs)

### Added
- Add `CdpError::CaptchaBlocked { detail }` variant for typed Google CAPTCHA/Sorry detection (error.rs)

## [0.5.0] - 

### Added
- Rewrote dismiss_allow_debugging_dialog() with proper sheet detection, "Allow" button click, 10 browser process names, and 20-retry polling loop (was catching main window instead of dialog sheet)
- Background dialog auto-accept handler subscribing to Page.javascriptDialogOpening events — auto-dismisses alert/confirm/prompt/beforeunload
- awaitPromise: true and timeout: 10000 to Runtime.evaluate params for async JS evaluation (required for SPA content extraction)

### Changed
- Made Connection::NEXT_CDP_ID pub(crate) and exposed write_tx() method for fire-and-forget CDP commands
- Added call_async() helper function for background CDP calls without response waiting

## 0.4.16 (2026-07-26)

### Changed
- Remove redundant `use tracing;` imports from connection.rs, browser.rs, tab.rs
- Remove stale `#[allow(dead_code)]` from connection.rs handle field
- Collapse intermediate Vec allocation in parse_signal_flags
- Fix stale doc comment (crate::parse_signal_flags → Session::parse_signal_flags)
- Add check_page_signals method for in-browser quality pre-check
- Add dispatch_message routing tests (response→oneshot, error→oneshot, event→broadcast)

## 0.4.15 (2026-07-26)

### Changes

- Change PendingCall, PendingMap, InternalMessage visibility to pub(crate)
- Minor doc comment fixes

## 0.4.14 (2026-07-25)

### Features

#### - New Session struct wrapping Connection with high-level API (create_tab, navigate, evaluate, wait_for, close_tab, disconnect)

- Stateless browser detection via detect(port) — probes HTTP endpoints and DevToolsActivePort files, never launches Chrome
- Event-driven Connection with broadcast channel for CDP events
- Fixed newWindow bug in Target.createTarget — tabs now open in existing window, not new OS windows
- Added window.close() via Runtime.evaluate before Target.closeTarget for reliable tab cleanup (Dia compatibility)
- Removed Browser lifetimes from tab methods — Tab::create and Tab::close now take &Session instead of &mut Connection
- Removed background parameter from Tab::create and Session::create_tab
- Added CdpEvent struct for event-driven patterns

## 0.4.13 (2026-07-25)

### Fixes

#### fix(cdp): stop deleting DevToolsActivePort in clean_profile_locks — this was destroying the file that discover_ws_url() relies on to find existing browsers

fix(cdp): add HTTP /json/version fallback in discover_ws_url() — detects browsers even when DevToolsActivePort is missing or at an unexpected path

## 0.4.12 (2026-07-25)

### Fixes

#### - Fix: `dismiss_allow_debugging_dialog()` now matches gsearch's exact approach — sends `keystroke return` to Dia process via osascript after 600ms delay, instead of attempting to click a button immediately

- Fix: `connect()` uses oneshot 600ms timer before dismissing dialog (dialog appears ~500ms after WS connect), matching `browser-harness-js/session.ts:189-197`

## 0.4.11 (2026-07-25)

### Fixes

#### - Fix: remove `--disable-extensions` flag — users can now have browser extensions active

- Fix: `dismiss_allow_debugging_dialog()` now matches actual dialog text and iterates over Dia, Chrome, Brave, Edge processes
- Fix: `connect()` spawns background task to dismiss dialog DURING WebSocket connection (dialog appears during WS handshake, not before)

## 0.4.10 (2026-07-25)

### Fixes

#### - Fix: tab creation for Dia — use `Target.attachToTarget` when `createTarget` returns no `sessionId` instead of failing via HTTP

- Fix: `find_existing()` now discovers WS URL from DevToolsActivePort across all common browser profile paths (Dia, Chrome, Brave, Edge)
- Fix: `handle_browser_status()` no longer falls back to `/tmp` for profile detection
- Add: `dismiss_allow_debugging_dialog()` — macOS AppleScript to auto-click "Allow" on "Allow debugging connection?" dialog
- Remove: `BrowserState`, `state_path()`, `pid()`, `home_dir()` — fully stateless
- Remove: `browser_state_path()` from CLI — no more `~/.gthings/browser.json`
- Test: 7 integration tests for DevToolsActivePort parsing, WS URL discovery, port probing
- Test: 4 E2E tests for stateless detection, no state file, dialog dismiss, content extraction

## 0.4.9 (2026-07-25)

### Fixes

#### - Fix: tab creation for Dia browser — when `Target.createTarget` returns `targetId` without `sessionId` (Dia CDP quirk), call `Target.attachToTarget` via CDP instead of falling back to HTTP (Dia doesn't support HTTP endpoints like `/json/new`)

- Remove: `create_via_http()` method — no longer needed since CDP attach works for all supported browsers
- Remove: `url` dependency usage — no HTTP URL parsing needed for tab creation

## 0.4.8 (2026-07-25)

### Fixes

#### - Fix: refactor profile resolution to match gsearch's proven approach

- Add: `resolve_profile()` — prefers real profile if not in use, falls back to seeded temp profile
- Add: `seed_profile()` — writes synthetic Preferences and Local State with `distribution.skip_first_run_ui: true` to suppress ALL first-run dialogs including Dia's Atlassian sign-in form
- Fix: `launch()` only cleans locks on real profiles — temp profiles skip lock cleaning entirely
- Fix: `is_profile_in_use()` prevents launching with a profile that has a running browser, avoiding crashes
- Remove: `real_profile_dir()` — replaced by `resolve_profile()` with proper in-use detection
- Remove: overlay dismissal JS from search/batch/follow extraction (unnecessary with proper profile seeding)

## 0.4.7 (2026-07-25)

### Fixes

#### - Fix: dismiss Dia sign-in overlay and other blocking dialogs via CDP JavaScript before page content extraction

- Add: overlay removal JS (onboarding, signin, login, aria-modal dialogs, Atlassian iframes) to search, batch, and follow extraction

## 0.4.6 (2026-07-25)

### Fixes

#### - Fix: add `--disable-fre` and `--disable-search-engine-choice-screen` launch flags to prevent first-run onboarding dialogs

- Fix: remove `--enable-automation` flag which triggered Google anti-bot measures and automation infobar
- Fix: add `--window-size=1280,720` for consistent viewport
- Fix: `find_existing()` now reads `DevToolsActivePort` file instead of HTTP `/json/version` (Dia browser doesn't support HTTP CDP endpoints)
- Fix: `launch()` checks if profile is in use before cleaning locks — prevents crashing user's real browser session
- Remove: `BrowserState` struct, `state_path()`, `pid()`, `fetch_ws_url()` — fully stateless, no `~/.gthings/browser.json`
- Add: `verify_ws()` for WebSocket-based browser aliveness check
- Add: `is_profile_in_use()` to detect running browser on profile directory

## 0.4.5 (2026-07-25)

### Features

#### - Refactor: fully stateless browser detection using DevToolsActivePort instead of PID state file

- Fix: redirect browser stderr to `/dev/null` (`Stdio::null()`) instead of piping and dropping — browser no longer exits after command completes
- Remove: `BrowserState` struct, `state_path()`, `pid()` — no more `~/.gthings/browser.json`
- Change: `find_existing()` now requires `profile_dir` parameter and uses DevToolsActivePort file + TCP probe + WebSocket verify instead of HTTP `/json/version` (Dia browser doesn't support HTTP CDP endpoints)
- Add: `wait_for_active_port()` polls `DevToolsActivePort` with exponential backoff; `verify_ws()` confirms browser is alive with `Browser.getVersion` CDP command

## 0.4.4 (2026-07-25)

### Fixes

- Fix: correct browser profile paths to include `/User Data` subdirectory so the browser finds its `Local State` and opens the real user profile (bookmarks, history, cookies preserved). Previous version tried `--profile-directory` which crashed Dia — removed.

## 0.4.3 (2026-07-25)

### Fixes

- Fix: correct browser profile paths to include `/User Data` subdirectory so `Local State` and profiles are found correctly
- Fix: add `detect_last_used_profile()` that reads `profile.last_used` from `Local State` to identify the user's actual profile (not just `Default`)
- Fix: add `--profile-directory` flag to launch command so the browser opens the real profile with bookmarks, history, and cookies instead of triggering onboarding

## 0.4.2 (2026-07-25)

### Fixes

- Fix: restore real profile detection to avoid browser onboarding/login prompts. Uses `real_profile_dir()` to find the user's real browser profile (Dia, Chrome, Arc, etc.) via macOS Launch Services detection.
- Fix: prevent SingletonLock conflicts by relying on `find_existing()` reuse — only one browser instance is ever launched per profile. If a fresh launch is needed, lock files are cleaned first.

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

