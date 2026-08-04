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
| `-q, --query <DOT>` | Custom dot-notation filter on JSON output (e.g. `.data`, `.[].url`, `.results[].snippet` — not full JMESPath) |
| `--cdp-port <PORT>` | CDP port (default: 9222) |
| `--cdp-url <URL>` | CDP WebSocket URL (overrides port) |
| `--timeout <SECS>` | Timeout for CDP/extraction (default: 30) |
| `-v` | Verbose (-v -v debug, -v -v -v trace) |
| `-q, --quiet` | Suppress non-error output |
| `--json` | Backward-compat alias for `--output json` |

## Output Envelope

Every command emits a `{status, data, error}` envelope:

```json
{
  "status": "ok" | "error",
  "data": <command-specific result>,
  "error": { "code": "ERROR_CODE", "detail": "...", "hint": "..." }
}
```

On success `status` is `"ok"` and `error` is `null`; on failure `status` is `"error"` and `data` is `null`. Use `--query .data` to unwrap the payload.

## Commands

### `gthings search <queries...> [--count N] [--strategy simple|parallel|harvest] [--engine auto|brave|bing|google] [--extract-results] [--max-chars N] [--dedup STR] [--rank STR] [--follow-top N] [--warn-tabs N]`

Google SERP search with strategy-based processing. Returns `{"results": [...], "query": "..."}` (simple), `{"results": [...]}` (parallel), or `{"results": [...], "summary": {...}}` (harvest). Default strategy: simple.

**Strategies:**
- `simple` (default): single-query search
- `parallel`: multi-query search, one entry per query (`{"ok": results}` or `{"error": ...}`)
- `harvest`: full pipeline — search → dedup → rank → select → follow → quality score → summary

**Engines (HTTP vs CDP):**
- `brave`, `bing`: plain-HTTP, no browser required
- `google`: requires a CDP browser (Chrome/Dia with `--remote-debugging-port=9222`)
- `auto` (default): uses a browser if available, otherwise degrades to HTTP engines

**Search operators:** `site:`, `-exclusion`, `"quoted"`, `filetype:`, `intitle:`, `inurl:`, `AROUND(n)`, `before:`/`after:`, `OR`/`AND`, `(...)`. Unsupported operators are stripped per-engine rather than failing.

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
