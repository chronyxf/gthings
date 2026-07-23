# Changelog — cdp

## 0.3.0 — 2026-07-23

## 0.3.1 — 2026-07-23

### Changed

- - Remove SKIP_CHECKS bypass from pre-commit hook
- - Run e2e tests serially (--test-threads=1) to avoid port conflicts
- - Run fmt, clippy, build, unit, integration, e2e checks on every code commit


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
