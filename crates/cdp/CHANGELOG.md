# Changelog — gthings-cdp

## 0.1.0 — 2026-07-23

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

### Features

- - Add new cdp crate: Browser launch, CDP connection, tab lifecycle
- - Add TraceWriter for step-level JSONL logging

### Changed

- - Remove legacy protocol/, cdp-core/, browser-daemon/ crates
- - Clean workspace Cargo.toml, .gitignore, .githooks


### Features
- Browser launch with resource cleanup (SingletonLock, SingletonSocket, SingletonCookie)
- Persistent browser mode (launch once, reuse across commands, port 9222)
- CDP WebSocket oneshot command dispatch
- Tab lifecycle: create via CDP or HTTP /json/new fallback, navigate, close
- Real Dia/Chrome profile with auto-detection
- content quality gate (score, is_ok, reasons)

### Fixes
- IPv4 + IPv6 port probing for Dia browser
- Wait for port 9222 to be free before launch (TIME_WAIT handling)
- Tab close: window.close() then Target.closeTarget (200ms delay)

### Changed
- Complete rewrite from browser-daemon per-command Chrome launch to persistent browser
