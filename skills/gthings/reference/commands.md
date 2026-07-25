# gthings CLI Command Reference

Every subcommand supports `--json` for structured JSON output.

**Browser requirement**: gthings does NOT manage Chrome. It detects an existing Chrome instance with remote debugging enabled on `GTHINGS_CDP_PORT` (default 9222). Run Chrome separately with `--remote-debugging-port=9222` or use another tool (Playwright, Puppeteer) to manage it.

## search — Google search

### `gthings search <query> [--count N] [--json]`

Single Google search. Returns `SearchResult[]`.

```
--count | Number of results (default: 5)
```

```json
[
  {"title":"Fed Holds Rates at 3.50-3.75%","url":"https://example.com/fed","snippet":"The Federal Reserve decided to hold...","position":1},
  {"title":"Market Reaction","url":"https://example.com/market","snippet":"Markets responded with...","position":2}
]
```

### Fields

| Field | Type | Description |
|-------|------|-------------|
| `title` | string | Result title |
| `url` | string | Result URL |
| `snippet` | string | Text snippet (may be empty) |
| `position` | number | Position in search results (1-based) |

## follow — Page content extraction

### `gthings follow <url> [--max-chars N] [--json]`

Extract text content from a web page. Returns `FollowResult`.

```
--max-chars | Max characters to extract (default: 15000)
```

```json
{
  "url":"https://example.com/article",
  "title":"Full Article Title",
  "content":"The extracted article text content...",
  "truncated":false,
  "error":""
}
```

On failure:

```json
{
  "url":"https://example.com/broken",
  "title":"",
  "content":"",
  "truncated":false,
  "error":"Navigation timeout"
}
```

### Fields

| Field | Type | Description |
|-------|------|-------------|
| `url` | string | The URL that was followed |
| `title` | string | Page title (from `document.title`) |
| `content` | string | Extracted page text |
| `truncated` | boolean | True if content was truncated to `--max-chars` |
| `error` | string | Empty string on success, error message on failure |

**Always check `truncated`** before processing content. If true, increase `--max-chars` to capture more text.

## batch — Multi-query search

### `gthings batch <queries...> [--count N] [--follow] [--max-chars N] [--json]`

Search multiple queries in a single command. Returns `SearchResult[][]` — one inner array per query, in the same order.

```
--count     | Results per query (default: 5)
--follow    | Best-effort URL reachability check (errors logged, not returned)
--max-chars | Max chars per follow when --follow is set
```

```json
[
  [
    {"title":"Result A1","url":"https://a1.com","snippet":"...","position":1},
    {"title":"Result A2","url":"https://a2.com","snippet":"...","position":2}
  ],
  [
    {"title":"Result B1","url":"https://b1.com","snippet":"...","position":1}
  ]
]
```

Each inner array corresponds to one query. With `--follow`, each result URL is visited (best-effort) to confirm reachability, but followed content is NOT retained in the output. Use separate `follow` calls if you need content.

## status — Check browser connection

### `gthings status [--json]`

Detect if a Chrome instance is available on the CDP port.

```json
{
  "status":"running",
  "ws_url":"ws://127.0.0.1:9222/devtools/browser/...",
  "browser":"Chrome",
  "version":"130.0.0.0"
}
```

When no browser is found:

```json
{
  "status":"stopped"
}
```

## Error codes

All errors are printed as JSON to stderr via `print_error()`:

```json
{"error":"BROWSER_NOT_FOUND","detail":"No browser found on port 9222","hint":"Open Chrome with --remote-debugging-port=9222"}
```

### Common error codes

| Code | Meaning | Fix |
|------|---------|-----|
| `BROWSER_NOT_FOUND` | No Chrome on CDP port | Start Chrome with `--remote-debugging-port=9222` |
| `CONNECTION_FAILED` | WebSocket rejected | Verify Chrome is running |
| `NAVIGATION_TIMEOUT` | Page timed out | Check network/URL |
| `SEARCH_FAILED` | CDP call failed | Retry with different arguments |
| `TAB_CREATE_FAILED` | Can't create tab | Check browser connection |
| `BATCH_FAILED` | Batch operation error | Retry with fewer queries |

## Exit codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | Operational error (error JSON printed to stderr) |
| 101+ | Panic (report as bug) |
