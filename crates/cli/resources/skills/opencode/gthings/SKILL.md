---
name: gthings
description: "Browser automation and web research CLI — search, follow, extract, PDF, batch, harvest with quality gates"
---

# Skill: gthings

Rust CLI for AI-agent-driven web research. Reuses installed Chrome via CDP. All commands produce JSON.

## When This Skill Activates

User says: "search" "research" "look up" "find" "gthings" "web research" "harvest" "extract" "pdf"

## Core Rules

1. Always use `--json` for structured output.
2. Prefer `harvest` for multi-query research (handles dedup, diversity, quality in one pass).
3. Filter results by `body_status == "ok"` before using content as factual source.
4. Check `quality.reasons` — never use content with paywall/bot_blocked/too_short.
5. Browser must be running with `--remote-debugging-port=9222`.

## Commands

### `gthings search <query> [--count N] [--json]`

Google SERP. Returns `SearchResult[]` with title, url, snippet, domain_authority, provenance.

### `gthings follow <url> [--max-chars N] [--offset N] [--json]`

Page content via CDP. Returns `FollowResult` with content, quality, pagination, sections.

### `gthings extract <url> [--max-chars N] [--offset N] [--json]`

HTTP extraction. Auto-detects web (Readability), PDF (pdftotext), arXiv (abs→pdf), GitHub (raw). Better for PDFs than follow.

### `gthings batch <q1> [<q2> ...] [--count N] [--follow] [--max-chars N] [--json]`

Multi-query search. Returns `SearchResult[][]`. With --follow, follows top result per query.

### `gthings harvest <q1> [<q2> ...] [--follow-top N] [--max-chars N] [--dedup url] [--rank composite] [--json]`

Full research pipeline: search → dedup → rank → diverse selection → follow → quality score → summary.

Output: `{ "results": HarvestedResult[], "summary": HarvestRunSummary }`

| Flag | Default | Description |
|------|---------|-------------|
| --follow-top | 8 | Max URLs to follow |
| --max-chars | 15000 | Max chars per follow |
| --dedup | url | Dedup strategy |
| --rank | composite | serp_order, domain_authority, snippet_length, composite |

### `gthings pdf url <url> [--json]` / `gthings pdf file <path> [--json]`

PDF via pdftotext. Requires `brew install poppler`. Quality >= 0.90 for clean text.

### `gthings status [--json]`

Browser connection check.

## Key Output Fields for Agent Triage

### body_status (in harvest results)

| Value | Agent Action |
|-------|-------------|
| ok | Use followed_content directly |
| pdf_unextracted | Fetch via extract or pdf command |
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

### coverage_by_query (in summary)

Per-query `{ total_hits, followed_ok, followed_failed }`. Shows which topics have body coverage.

### warnings

`follow_budget_collapsed_to_one_site`, `no_body_for_query:<q>`, `all_snippet_only`

## Error Handling

| Signal | Action |
|--------|--------|
| BROWSER_NOT_FOUND | Start Chrome with --remote-debugging-port=9222 |
| CONNECTION_FAILED | Check status, verify port |
| body_status=chrome_or_empty | Retry with extract instead of follow |
| body_status=pdf_unextracted | Use extract or pdf command |
| quality.score==0 with reasons | Skip result |
