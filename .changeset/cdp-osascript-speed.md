gthings-cdp: patch

- Fix: reduce osascript dialog dismissal timeout (5s to 1s, 2 attempts to 1)
- Fix: try WebSocket connection first, skip osascript when dialog not present
- Perf: reduce per-command startup overhead from ~1.6s to ~0s in common case
