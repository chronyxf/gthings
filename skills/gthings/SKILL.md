---
name: gthings
description: "Web research toolkit — search, read, extract PDFs, batch harvest, quality gate, agent telemetry. Use ONLY for web research and web scraping — not local file ops or system admin."
compatibility: "Rust 1.85+, Chrome or Chromium browser"
---

# Skill: gthings

Browser automation and web research toolkit. Native Rust binary — single static binary that launches Chrome on demand.

## Quick Reference

```bash
# Search
gthings --json search query "<topic>" --count 10            # Single Google search
gthings --json search batch "q1" "q2" --count 5             # Multi-query (dedup by URL)
gthings --json search harvest "q1" "q2" --count 5 --max 3   # Two-phase: search + follow top results

# Read pages
gthings --json follow url "<url>" --max 20000                # Single page
gthings --json follow batch "url1" "url2" --max 20000       # Batch (3+ URLs)

# PDFs
gthings --json pdf url "<arxiv-url>"                         # arXiv/PDF extraction
gthings --json pdf file "<path>"                             # Local PDF extraction

# Browser lifecycle
gthings browser start                                        # Start persistent browser
gthings browser stop                                         # Stop browser
gthings browser status                                       # Check if running

# Telemetry
gthings --trace /tmp/run.jsonl --json search query "topic"   # Record all commands
```

## Core Rules

1. **Always use `--json`** — every command outputs structured JSON.
2. **Prefer `search harvest`** for multi-topic research — one command replaces search×N + follow×M.
3. **Use `--trace <file>`** to record all commands for observability.
4. **Batch independent follows** — use `follow batch` for 3+ URLs.
5. **Expect ~2s cold start** — first call launches Chrome. Subsequent calls reuse it.

## Architecture

```
AI Agent → gthings CLI (binary, ~7MB)
              ├── parses command
              ├── launches/manages Chrome subprocess (headless, port 9222)
              ├── connects via Chrome DevTools Protocol (CDP)
              ├── executes operation (search/follow/extract)
              ├── returns JSON result
              └── browser stays alive for reuse (persistent)
```

- **CLI**: Parses commands, manages Chrome lifecycle, formats JSON output, writes traces.
- **cdp** (internal crate): Chrome process management, WebSocket transport, CDP command dispatch.
- **extraction**: HTML content extraction, PDF text extraction, quality validation.
- **search/search**: Google search via CDP JavaScript evaluation.
- **search/follow**: Page following with caching, section detection, quality gates.

**Persistent browser**: Chrome stays alive between commands (port 9222). First call launches, subsequent calls reuse. `browser stop` to kill it.

## JSON Return Types

| Command | Top-level fields |
|---------|-----------------|
| `search query` | `{meta: {total, query, duration_ms}, results: [{title, url, snippet, query}]}` |
| `search harvest` | `{search_results: [...], read_pages: [...], meta: {queries, total_search_results, unique_urls, pages_followed, pages_skipped, duration_ms}}` |
| `follow url` | `{success, url, content, total_length, offset, truncated, sections: [{heading, content}], error, quality: {score, is_ok, reasons, length}}` |
| `pdf url` | `{source, text, length, pages}` |
| `browser status` | `{status: "running"|"stopped", pid, ws_url}` |

**Always check `quality.is_ok`** before processing followed content.

## Key Design Decisions

| Decision | Rationale |
|----------|-----------|
| **Persistent browser** (not per-call) | ~2s Chrome startup is amortized across commands. Single browser process is simpler than daemon + socket. |
| **`--json` as global flag** | Default is human-readable. `--json` for machine parsing. |
| **`search harvest` two-phase** | Phase 1 searches all queries, Phase 2 follows top results. One CLI invocation replaces 12+ commands. |
| **15k char default cap** | Most pages have diminishing returns beyond 15k chars. Use `--max 50000` for long-form content. |
| **Pure Rust PDF** | No external PDF library. Works with raw bytes + regex. arXiv /abs/ URLs auto-rewritten. |

## Traps

1. **Chrome cold start**: First call takes ~2s. Use `browser status` to check before working.
2. **Empty search results**: Google rate-limiting or network. Retry with different wording.
3. **Low quality content** (`quality.is_ok=false`): Retry with `--selector "body" --max 30000`.
4. **Truncated content** (`truncated: true`): Increase `--max` (up to 100000) or use `--offset`.
5. **Empty sections** (`sections: []`): Page lacks h1/h2/h3. Content is still in `content` field.
6. **PDF URL not PDF**: Use `follow url` for HTML pages that aren't PDFs.
7. **Browser not stopping**: `browser stop` kills the process. If stuck, `kill <pid>` manually.

## Environment Variables

| Variable | Default | Purpose |
|----------|---------|---------|
| `GTHINGS_CACHE_DIR` | `/tmp/gthings-cache` | Disk cache directory |
| `GTHINGS_CACHE_TTL_SECS` | `3600` | Cache TTL in seconds |
| `GTHINGS_CDP_PORT` | `9222` | Chrome remote debugging port |
| `GTHINGS_LOG_LEVEL` | `info` | Tracing/log level |
| `GTHINGS_PER_HOST_RATE` | `2` | Steady-state requests/sec per host |
| `GTHINGS_PER_HOST_BURST` | `5` | Max burst per host |

## Reference Docs

- `reference/commands.md` — every CLI command with flags and JSON return types
- `reference/quality.md` — content quality gate, section extraction, bot/captcha/paywall detection
- `reference/errors.md` — error codes, troubleshooting
- `reference/agent-trace.md` — `--trace` telemetry format
- `reference/agent-prompt.md` — prompt template for AI agents
