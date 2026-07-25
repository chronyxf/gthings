---
name: gthings
description: Browser-based web research toolkit. Native Rust binary — detects an existing Chrome with remote debugging, never launches one.
version: 2.0.0
model: deepseek-v4-flash
temperature: 0.2
steps: 3
permission: allowed
---

# Skill: gthings

Browser detection + search + follow — stateless per-command cycle: detect → connect → operate → close.

## Core Rules

1. **Always use `--json`** — every subcommand outputs structured JSON.
2. **Prefer `batch`** for multi-query research — one command replaces sequential `search` calls.
3. **Browser is detected-only** — gthings never launches Chrome. It connects to an existing instance on `GTHINGS_CDP_PORT` (default 9222). `status` checks if one is running.
4. **Stateless** — each command opens a fresh CDP connection, operates, then disconnects. No daemon, no cache, no session.
5. **Check `truncated`** flag in `FollowResult` before processing content — if true, increase `--max-chars`.

## Commands

### `search` — Google search, returns `SearchResult[]`

```bash
gthings search <query> [--count N] [--json]
```

```json
[
  {"title":"Example","url":"https://example.com","snippet":"This is an example.","position":1},
  {"title":"Foo","url":"https://foo.com","snippet":"Foo bar.","position":2}
]
```

Fields: `title`, `url`, `snippet`, `position`.

### `follow` — Extract page content, returns `FollowResult`

```bash
gthings follow <url> [--max-chars N] [--json]
```

```json
{
  "url":"https://example.com",
  "title":"Example Domain",
  "content":"Example domain text content...",
  "truncated":false,
  "error":""
}
```

Fields: `url`, `title`, `content`, `truncated`, `error`. Default `--max-chars` is 15000.

### `batch` — Multi-query search, returns `SearchResult[][]`

```bash
gthings batch <queries...> [--count N] [--follow] [--max-chars N] [--json]
```

```json
[
  [{"title":"A","url":"...","snippet":"...","position":1}, ...],
  [{"title":"B","url":"...","snippet":"...","position":2}, ...]
]
```

- `--follow`: after search, each result URL is fetched best-effort to verify reachability. Errors are logged (not returned). The output format is unchanged — still `SearchResult[][]`. Followed content is not retained in output.
- Each inner array corresponds to one query, in the same order.

### `status` — Check browser connection

```bash
gthings status [--json]
```

```json
{
  "status":"running",
  "ws_url":"ws://127.0.0.1:9222/devtools/browser/...",
  "browser":"Chrome",
  "version":"130.0.0.0"
}
```

The `browser` and `version` fields come from `DetectedBrowser`.

## Error Handling

| Error Code | Cause | Fix |
|------------|-------|-----|
| `BROWSER_NOT_FOUND` | No Chrome listening on CDP port | Start Chrome with `--remote-debugging-port=9222` |
| `CONNECTION_FAILED` | WebSocket connection rejected | Verify Chrome is running and port is accessible |
| `NAVIGATION_TIMEOUT` | Page or search took too long | Retry or check network |
| `SEARCH_FAILED` | CDP call failed during search | Retry with different arguments |
| `TAB_CREATE_FAILED` | Could not create browser tab | Check browser connection |
| `BATCH_FAILED` | Batch operation error | Retry with fewer queries or longer timeout |
| `null` / empty array | Search returned no results | Retry with different query wording |
| `truncated: true` | Content exceeded `--max-chars` | Increase `--max-chars` |
| `error` non-empty in `FollowResult` | Follow failed for that URL | Check URL format or page accessibility |
| WebSocket disconnected | Chrome closed or crashed | Restart Chrome and retry |

## Traps

1. **Browser must already be running** — gthings never starts Chrome. Run Chrome separately with `--remote-debugging-port=9222` or have another automation tool (Playwright, Puppeteer) manage it.
2. **First call is ~500ms slower** — Rust CDP connection negotiation. Subsequent calls reuse nothing (stateless), so each call pays this cost.
3. **`--count` default is 5** — not 10 as common search tools. Explicitly set `--count 10` if you need more.
4. **`--max-chars` cuts by char count, not element** — content truncation may occur mid-word. Always check the `truncated` boolean.
5. **Flat subcommands only** — `gthings search batch` is invalid. Use `gthings batch` directly.
6. **URLs must be fully qualified** — include scheme (`https://`). Relative URLs return error.
7. **No PDF extraction** — removed. Use `follow` if the page renders text in the DOM, or a dedicated PDF tool.

## Environment Variables

| Variable | Default | Purpose |
|----------|---------|---------|
| `GTHINGS_CDP_PORT` | `9222` | Chrome remote debugging port |

All other environment variables from earlier versions (`GTHINGS_CACHE_DIR`, `GTHINGS_CACHE_TTL_SECS`, `GTHINGS_LOG_LEVEL`, etc.) are **no longer supported**.
