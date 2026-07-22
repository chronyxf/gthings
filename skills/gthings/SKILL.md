---
name: gthings
description: "Web research toolkit — search, read, extract PDFs, screenshot, scrape. Multi-topic batch harvest, page content with section detection, quality gate, and agent telemetry. Persistent daemon, zero runtime dependencies. Use ONLY for web research — not local file ops or system admin."
compatibility: "Rust 1.85+, Chrome or Chromium browser"
---

# Skill: gthings

Browser automation and web research toolkit. Native Rust binary — no shell, no TypeScript, no bun. Single static binary with a persistent daemon for all browser operations.

## When This Skill Activates

Use this skill when the task requires:
- Searching the web for information on a topic
- Reading/extracting content from a web page
- Capturing screenshots or scraping data
- Extracting text from PDFs (research papers, reports)
- Multi-topic research (e.g., 3+ queries with follow)
- Any web research that needs structured JSON output for AI consumption

Do NOT use this skill for:
- Local file operations
- Non-web data sources (APIs, databases)
- Simple lookups where `webfetch` is sufficient

## The Core Rule

1. **Always use `--json`** — every command outputs structured JSON. Agents parse JSON, not terminal text.
2. **Start the daemon first** — `gthings browser start --port 9222` before any search/follow/screenshot/scrape.
3. **Prefer `search harvest`** for multi-topic research — one command replaces search×N + follow×M.
4. **Use `--trace <file>`** to record all commands for observability and debugging.
5. **Batch independent follows** — use `follow batch` for 3+ URLs instead of individual `follow url` calls.

## Required First Pass

```bash
# 1. Start the daemon (one-time, persists)
gthings browser start --port 9222

# 2. Verify it's running
gthings --json browser status

# 3. Run research
gthings --trace /tmp/research.jsonl --json search harvest "topic1" "topic2" --count 5 --max 3
```

## Architecture

```
AI Agent ──→ gthings CLI (binary, 27MB)
                │
                ├── UDS socket (/tmp/gthings-daemon.sock)
                │
                └── browser-daemon (persistent process, 21MB)
                     │
                     ├── cdp-core (WebSocket → Chrome DevTools Protocol)
                     ├── cdp-protocol (generated 652 typed CDP methods)
                     └── Chrome/Dia Browser (headless, port 9222)
```

- **CLI**: Parses commands, formats output, writes traces. No browser logic.
- **Daemon**: Persistent process. Manages CDP connection, tab lifecycle, data extraction. One daemon = many CLI calls.
- **cdp-core**: WebSocket transport, oneshot command dispatch, flattened sessionId routing.
- **cdp-protocol**: Generated from Chrome DevTools Protocol JSON — 56 domains, 652 commands.
- **Browser**: Any Chromium-based (Chrome, Dia, Edge, Brave, Arc, Opera, Vivaldi). Dia has quirks handled automatically.

## Reference Documents

| Document | Covers |
|----------|--------|
| `reference/commands.md` | Every CLI command with flags, JSON return types, examples |
| `reference/daemon.md` | Daemon lifecycle, UDS protocol, port management |
| `reference/quality.md` | Content quality gate, section extraction, bot/captcha/paywall detection |
| `reference/errors.md` | Error codes, troubleshooting, common failure patterns |
| `reference/agent-trace.md` | `--trace` telemetry format, fields, analysis |
| `reference/agent-prompt.md` | Full prompt template for AI agents using gthings |

## Design Rationale

**Why Rust?** Single static binary. Zero runtime dependencies. Full type safety across 7 crates. The old system needed bash + bun + Node.js — 3 runtimes. gthings needs none.

**Why a daemon?** Persistent CDP connection eliminates per-command startup overhead. First follow takes ~3s (tab create + navigate + poll). Subsequent follows to the same daemon reuse the connection, making batch operations faster.

**Why `--json` as a global flag?** Every command returns human-readable output by default and machine-readable JSON with `--json`. This is the reverse of the old system which returned JSON by default for some commands and plain text for others.

**Why `search harvest`?** Two-phase pipeline: Phase 1 searches all queries in parallel tabs, Phase 2 follows top results. One CLI invocation does what would take 12+ individual commands.

**15k character default cap** is based on information-theoretic analysis (Liu et al. 2023): most web pages have diminishing returns beyond 15,000 characters of extracted text. Use `--max 50000` for long-form content.

## Playbook

1. **Start daemon**: `gthings browser start --port 9222`
2. **Enable telemetry**: Add `--trace /tmp/run.jsonl` to every command
3. **Discover**: For single topic: `search query "topic" --count 5`. For multi-topic: `search harvest "q1" "q2" "q3" --count 5 --max 3`
4. **Read**: `follow url <url> --max 20000`. Check `quality.is_ok` before processing content.
5. **Batch reads**: For 3+ URLs: `follow batch "url1" "url2" "url3" --max 20000`
6. **Extract papers**: `pdf url <arxiv-url>` for research papers. `pdf file <path>` for local PDFs.
7. **Extract data**: `scrape <url> --selector "table"` for structured data from tables.
8. **Capture visuals**: `screenshot <url> --json` returns base64 PNG for vision-capable agents.
9. **Refine**: If results are sparse, try `search query` with different wording.
10. **Synthesize**: Compile findings from followed pages. Use `sections` array for document structure.
11. **Analyze trace**: After research, parse the trace file to understand tool usage patterns.

## The Traps You Will Hit

**Trap 1: Daemon not running**
`search`/`follow`/`screenshot`/`scrape` all fail with "daemon not connected" if the daemon isn't running.
→ Run `gthings browser start --port 9222` first. Check with `gthings browser status`.

**Trap 2: Empty search results**
Google sometimes returns empty results due to rate-limiting or network issues.
→ `search harvest` auto-retries with trailing-space query. For `search query`, retry with different wording.

**Trap 3: Low quality content**
`follow` returns `quality.is_ok=false` when content is too short, is a paywall, or is an error page.
→ Retry with `--selector "body" --max 30000`. If still failing, the page genuinely has no useful content.

**Trap 4: PDF URL is not a PDF**
Agent constructs wrong PDF URL (e.g., guessing IMF PDF path).
→ Error now says "HTTP 404" or "does not appear to be a PDF" with URL + guidance. Use `follow url` instead.

**Trap 5: Truncated content**
Content ends at `--max` limit. `truncated: true` indicates more content exists.
→ Increase `--max` (up to 100000). Or use `--offset` to read subsequent chunks.

**Trap 6: Sections are empty**
`sections: []` means no h1/h2/h3 were found on the page. Not all pages use semantic headings.
→ The `content` field still has the full text. Sections are a best-effort enhancement, not a guarantee.

**Trap 7: Slow first follow**
The first `follow` call takes ~3s because it creates a tab + navigates + polls readyState.
→ Subsequent calls to the same daemon reuse the connection and are faster. Use `follow batch` for multiple URLs.

**Trap 8: Screenshot with --json doesn't write file**
`--json` outputs base64 JSON. `--output` is ignored when `--json` is set.
→ Use `screenshot <url>` (no --json) to write a file. Use `screenshot --json <url>` to get base64 for agents.

## Decision Tree

```
Task: Research a topic
├── Single fact → gthings --json search query "fact query" --count 3
├── Broad topic, 3+ subtopics
│   └── gthings --json search harvest "sub1" "sub2" "sub3" --count 5 --max 3
│
Task: Read a page
├── URL is HTML → gthings --json follow url "<url>" --max 20000
├── URL is PDF
│   ├── arXiv (arxiv.org/abs/...) → gthings --json pdf url "<url>"
│   └── Other PDF → gthings --json follow url "<url>" (Chrome viewer)
│
Task: Extract data
├── Table → gthings --json scrape "<url>" --selector "table"
├── Headings → gthings --json scrape "<url>" --selector "h1,h2,h3" --attr textContent
└── Links → gthings --json scrape "<url>" --selector "a[href]" --attr href
│
Task: Capture visual
└── gthings --json screenshot "<url>" --json
│
Task: Read 5+ pages
└── gthings --json follow batch "url1" "url2" "url3" "url4" "url5" --max 20000
```

## Rationalization Table

| You will think | Don't |
|---|---|
| "I'll just read the URL with webfetch" | Use `follow url` — it runs JS, handles redirects, extracts clean text with sections |
| "I'll estimate the content length" | Set `--max 50000` if unsure — the quality gate tells you if content is garbage |
| "I'll skip the daemon for one quick search" | Daemon startup is 5s. A single search is 2s. Total: 7s. Worth it. |
| "I'll use `--json` only when I need it" | Always use `--json`. Human-readable mode is for debugging. Agents need JSON. |
| "I'll make 5 separate follow calls" | Use `follow batch` — same result, one CLI invocation, faster. |
| "I'll read a PDF by following its URL" | Use `pdf url` for arXiv. For other PDFs, `follow url` uses Chrome's PDF viewer. |
| "I don't need --trace for a quick task" | `--trace` adds no overhead. Add it by default — you can't analyze what you didn't record. |
| "Search harvest looks complex, I'll search then follow manually" | `search harvest` does both phases in one call. 12× fewer commands for multi-topic work. |
| "This section is empty, follow must be broken" | Some pages don't have h1/h2/h3. Content is still in the `content` field. |

## Checklist

Before marking a research task complete:
- [ ] Daemon is running (`browser status` returns ok)
- [ ] Used `--json` for all commands
- [ ] Used `--trace` to record tool usage
- [ ] Checked `quality.is_ok` before processing content
- [ ] Checked `truncated` flag — is content complete?
- [ ] For multi-topic: used `search harvest` (not individual search+follow)
- [ ] For 3+ URLs: used `follow batch` (not individual follow calls)
- [ ] PDF errors checked against actual URL (not guessed path)
- [ ] Trace analyzed for tool usage patterns
- [ ] Daemon stopped if research is complete

## Environment Variables

| Variable | Default | Purpose |
|----------|---------|---------|
| `GTHINGS_DAEMON_SOCKET` | `/tmp/gthings-daemon.sock` | UDS socket path for CLI-daemon communication |
| `GTHINGS_CACHE_DIR` | `/tmp/gthings-cache` | Disk cache directory |
| `GTHINGS_CACHE_TTL_SECS` | `3600` | Cache TTL in seconds |
| `GTHINGS_REQUEST_TIMEOUT_MS` | `30000` | HTTP request timeout for PDF downloads |
| `GTHINGS_LOG_LEVEL` | `info` | Tracing/log level (debug, info, warn, error) |

## Testing

```bash
# Integration tests (no daemon needed)
cargo test --test integration

# E2E tests (needs daemon)
GTHINGS_TEST_DAEMON=1 cargo test --test e2e -- --ignored

# All tests
cargo test
```

25 integration tests + 12 e2e tests + 33 unit tests = 70 tests.
