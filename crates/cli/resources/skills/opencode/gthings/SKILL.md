---
name: gthings
description: "Browser automation and web research CLI — search, extract, PDF, AX, status, update with quality gates"
---

# Skill: gthings

Rust CLI for AI-agent-driven web research. Reuses installed Chrome via CDP. All commands produce JSON.

## When This Skill Activates

User says: "search" "research" "look up" "find" "gthings" "web research" "extract" "pdf" "ax"

## Core Rules

1. Always use `--output json` for structured output (or `--json` as backward-compat alias).
2. Prefer `search --strategy harvest` for multi-query research (handles dedup, diversity, quality in one pass).
3. Filter results by `body_status == "ok"` before using content as factual source.
4. Check `quality.reasons` — never use content with paywall/bot_blocked/too_short.
5. Browser must be running with `--remote-debugging-port=9222`.

## Universal Flags (every command)

| Flag | Description |
|------|-------------|
| `-o, --output <FORMAT>` | Output format: text, json, nd-json (default: text) |
| `-q, --query <JMES>` | JMESPath filter on JSON output |
| `--cdp-port <PORT>` | CDP port (default: 9222) |
| `--cdp-url <URL>` | CDP WebSocket URL (overrides port) |
| `--timeout <SECS>` | Timeout for CDP/extraction (default: 30) |
| `-v` | Verbose (-v -v debug, -v -v -v trace) |
| `-q, --quiet` | Suppress non-error output |
| `--json` | Backward-compat alias for `--output json` |

## Commands

### `gthings search <queries...> [--count N] [--strategy simple|parallel|harvest] [--extract-results] [--max-chars N] [--dedup STR] [--rank STR] [--follow-top N] [--warn-tabs N]`

Google SERP search with strategy-based processing. Returns `SearchResult[]` (simple), `SearchResult[][]` (parallel), or `HarvestedResult[]` with summary (harvest). Default strategy: simple.

### `gthings extract <url> [--max-chars N] [--offset N]`

HTTP extraction. Auto-detects web (Readability), PDF (pdftotext), arXiv (abs→pdf), GitHub (raw). Returns `Article` with quality score.

### `gthings ax <url> [--max-nodes N]`

Fetch compressed accessibility tree via CDP. Returns AX nodes (default max 500). Good for structured page analysis.

### `gthings pdf-url <url> [--max-chars N] [--offset N]`

PDF extraction via pdftotext from URL. Requires `brew install poppler`. Quality >= 0.90 for clean text.

### `gthings pdf-file <path> [--max-chars N] [--offset N]`

PDF extraction from local file.

### `gthings status`

Browser connection check.

### `gthings update`

Update gthings to latest version.

## Key Output Fields for Agent Triage

### body_status (in harvest results)

| Value | Agent Action |
|-------|-------------|
| ok | Use followed_content directly |
| pdf_unextracted | Fetch via extract or pdf-url command |
| extract_failed | Skip — blocked/paywall |
| chrome_or_empty | Skip — no usable content |
| snippet_only | Lead only, not a body |

### quality.score

| Range | Agent Decision |
|-------|---------------|
| >= 0.80 | Use directly |
| 0.50-0.79 | Use but verify claims |
| < 0.50 | Skip or corroborate |

### quality.reasons to reject content

`paywall`, `bot_blocked`, `captcha`, `empty_shell`, `too_short`, `too_few_words`, `low_entropy`, `empty_content`

### coverage_by_query (in harvest summary)

Per-query `{ total_hits, followed_ok, followed_failed }`. Shows which topics have body coverage.

### warnings

`follow_budget_collapsed_to_one_site`, `no_body_for_query:<q>`, `all_snippet_only`

## Error Handling

| Signal | Action |
|--------|--------|
| BROWSER_NOT_FOUND | Start Chrome with --remote-debugging-port=9222 |
| CONNECTION_FAILED | Check status, verify port |
| body_status=chrome_or_empty | Retry with extract instead |
| body_status=pdf_unextracted | Use extract or pdf-url |
| quality.score==0 with reasons | Skip result |
