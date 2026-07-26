---
gthings-cdp: patch
---

- Remove redundant `use tracing;` imports from connection.rs, browser.rs, tab.rs
- Remove stale `#[allow(dead_code)]` from connection.rs handle field
- Add `check_page_signals` method for in-browser quality pre-check
- Add dispatch_message routing tests (response→oneshot, error→oneshot, event→broadcast)
- Fix stale doc comment reference (crate::parse_signal_flags → Session::parse_signal_flags)
- Collapse intermediate Vec<String> allocation in parse_signal_flags
