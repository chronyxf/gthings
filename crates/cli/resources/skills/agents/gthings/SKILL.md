---
name: gthings
description: "Browser automation and web research CLI — search, follow, extract, PDF, batch, harvest with quality gates"
---

# Skill: gthings

Rust CLI for AI-agent-driven web research. Reuses installed Chrome via CDP. All commands produce JSON.

## When This Skill Activates

User says: "search" "research" "look up" "find" "gthings" "web research" "harvest" "extract" "pdf"

## Core Rules

1. Always use `--json` for structured output an AI agent can parse.
2. Prefer `harvest` for multi-query research (handles dedup/diversity/quality in one pass).
3. Filter results by `body_status == "ok"` before using content.
4. Check `quality.reasons` — never use content with paywall/bot_blocked/too_short.

## Commands

### `gthings search <query> [--count N] [--json]`

Google SERP search. Returns results with title, url, snippet, domain_authority, provenance.

### `gthings follow <url> [--max-chars N] [--offset N] [--json]`

Single page extraction via CDP. Content from `document.body.innerText`. Check `quality` field.

### `gthings extract <url> [--max-chars N] [--offset N] [--json]`

HTTP-based extraction. Auto-detects web (Readability), PDF (pdftotext), arXiv (abs→pdf), GitHub (raw). Better for PDF/academic content.

### `gthings batch <q1> [<q2> ...] [--count N] [--follow] [--max-chars N] [--json]`

Multi-query search. With --follow, follows top result per query.

### `gthings harvest <q1> [<q2> ...] [--follow-top N] [--max-chars N] [--dedup url] [--rank composite] [--json]`

Full research pipeline: search all queries → dedup (URL normalization, fragment/tracking-param stripping) → rank (composite score: authority + snippet + diversity) → select (per-query minimum, per-host cap, junk filter) → follow → quality score.

Output: `{ "results": HarvestedResult[], "summary": HarvestRunSummary }`

| Flag | Default | Description |
|------|---------|-------------|
| --follow-top | 8 | Max URLs to follow |
| --max-chars | 15000 | Max chars per follow |
| --dedup | url | Dedup strategy |
| --rank | composite | serp_order, domain_authority, snippet_length, composite |

### `gthings pdf url <url> [--json]` / `gthings pdf file <path> [--json]`

PDF extraction via pdftotext. Requires poppler (`brew install poppler`). Quality >= 0.90 for clean scientific text.

### `gthings status [--json]`

Browser connection check.

## Key Output Fields for Agent Triage

### body_status

| Value | Meaning | Agent Action |
|-------|---------|-------------|
| ok | Full body extracted | Use `followed_content` |
| pdf_unextracted | PDF/arXiv, CDP can't handle | Fetch via `extract <url>` or `pdf url <url>` |
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

### coverage_by_query (in HarvestRunSummary)

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
| body_status=chrome_or_empty | Nav-only page | Retry with extract instead of follow |
| body_status=pdf_unextracted | PDF not extracted | Use extract or pdf command |
| quality.score==0 with reasons | Complete failure | Skip result |
