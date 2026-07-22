# Changelog

All notable changes to this project will be documented in this file.

## 0.1.0 — 2026-07-22

### Minor Changes

- Add DaemonContext struct and get_context RPC for browser identity introspection
- Add browser type detection (Chrome/Dia) with DevToolsActivePort fallback
- Add browser PID extraction from DevToolsActivePort on discovery path
- Add JSONL daemon log at /tmp/gthings-daemon.log
- Enrich --trace JSONL records with daemon context (daemon PID, browser type/version/PID, CDP port)
- Fix browser status to include browser_pid, browser_type, connection_method

## 0.1.0 — 2026-07-22
### Minor Changes
- Add SessionPool for concurrent CDP tab reuse in cdp-core
- Add batch RPC handlers for search.batch, follow.batch, and harvest to daemon
- Rewrite search harvest to use single daemon RPC instead of per-query UDS loop
- Improve SEARCH_JS with organic block detection and deny_hosts filtering
- Add --concurrency and --follow-concurrency CLI flags to search harvest
- Add search_concurrency, follow_concurrency, max_chars, deny_hosts config
- Benchmark: 29-35% faster after batch refactor
### Patch Changes
- Move cdp-protocol crate from crates/ to protocol/ directory
- Pre-generate cdp.rs, add protocol/generated/ to .gitignore
- JSON protocol files now self-contained in protocol/ (not skills/cdp/sdk/)
- Remove stale skills/cdp/ directory
---
- Migrate from shell+TypeScript to native Rust (7 crates, static binary)
- Add persistent daemon with UDS protocol replacing bun subprocesses
- Add --trace flag for per-command JSONL agent telemetry
- Add screenshot --json base64 output for vision-capable agents
- Add pdf url structured error messages (HTTP status, content type hints)
- Add follow sections extraction and harvest unique_urls/pages_skipped
- Fix CDP session_id camelCase serialization and sections pass-through
- Add 37 tests, clippy+rustfmt config, and gthings skill docs
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
