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
2. Prefer `search --strategy harvest` for multi-query research (handles dedup/diversity/quality in one pass).
3. Filter results by `body_status == "ok"` before using content.
4. Check `quality.reasons` — never use content with paywall/bot_blocked/too_short.

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

HTTP-based extraction. Auto-detects web (Readability), PDF (pdftotext), arXiv (abs→pdf), GitHub (raw). Returns `Article` with quality score.

### `gthings ax <url> [--max-nodes N]`

Fetch compressed accessibility tree via CDP. Returns AX tree nodes (default max 500). Useful for structured page analysis without full render.

### `gthings pdf-url <url> [--max-chars N] [--offset N]`

PDF extraction via pdftotext from URL. Requires poppler (`brew install poppler`). Quality >= 0.90 for clean scientific text.

### `gthings pdf-file <path> [--max-chars N] [--offset N]`

PDF extraction from local file.

### `gthings status`

Browser connection check.

### `gthings update`

Update gthings to latest version.

## Key Output Fields for Agent Triage

### body_status

| Value | Meaning | Agent Action |
|-------|---------|-------------|
| ok | Full body extracted | Use `followed_content` |
| pdf_unextracted | PDF/arXiv, CDP can't handle | Fetch via `extract <url>` or `pdf-url <url>` |
| extract_failed | Paywall/bot/network error | Skip |
| chrome_or_empty | Nav-only or empty page | Skip |
| snippet_only | Not followed, SERP only | Lead only |

### quality.score

| Range | Meaning | Agent Decision |
|-------|---------|---------------|
| >= 0.80 | Clean text | Use directly |
| 0.50-0.79 | Readable with issues | Use but verify claims |
| < 0.50 | Low quality | Skip or corroborate |

### quality.reasons (non-empty when low)

`paywall`, `bot_blocked`, `captcha`, `empty_shell`, `too_short`, `too_few_words`, `low_entropy`, `empty_content`

If any of these are present, do NOT use the content as factual source.

### coverage_by_query (in harvest summary)

Map of query -> `{ total_hits, followed_ok, followed_failed }`. Use this to identify which topics have body coverage.

### warnings

`follow_budget_collapsed_to_one_site`, `no_body_for_query:<query>`, `all_snippet_only`

## URL Canonicalization (applied to harvest)

- Scheme+host lowercased
- Tracking params removed: utm_*, fbclid, gclid, _ga, _gl, mc_cid, mc_eid
- Fragment stripped for dedup keys
- Path lowercased, trailing slash stripped
- Query params sorted alphabetically

## Error Handling

| Signal | Cause | Action |
|--------|-------|--------|
| BROWSER_NOT_FOUND | Chrome not running with --remote-debugging-port=9222 | Start browser |
| CONNECTION_FAILED | CDP port wrong | Check with status |
| body_status=chrome_or_empty | Nav-only page | Retry with extract instead |
| body_status=pdf_unextracted | PDF not extracted | Use extract or pdf-url |
| quality.score==0 with reasons | Complete failure | Skip result |
