# Changelog

All notable changes to this project will be documented in this file.

## 0.1.0 — 2026-07-22

### Minor Changes

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
