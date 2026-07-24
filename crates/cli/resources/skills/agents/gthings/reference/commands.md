# gthings CLI Command Reference

Every command supports `--json` (global, structured JSON output) and `--trace <file>` (telemetry).

Chrome is auto-launched on first use and stays alive (persistent on port 9222). No manual browser management needed — but use `browser start`, `browser stop`, `browser status` to control it explicitly.

## Search

### `gthings --json search query "<query>" --count N`

Single Google search.

```
--count | Number of results (default: 10)
```

```json
{
  "meta": {"total": 5, "query": "fed rate 2026", "duration_ms": 2340},
  "results": [
    {"title": "Fed Holds Rates at 3.50-3.75%", "url": "https://...", "snippet": "...", "query": "fed rate 2026"}
  ]
}
```

### `gthings --json search batch "q1" "q2" --count N`

Multi-query search. Results deduplicated by URL.

```json
{
  "results": [...],
  "meta": {"total": 8, "query": "q1, q2", "duration_ms": 4100}
}
```

### `gthings --json search harvest "q1" "q2" --count N --max M`

**Two-phase pipeline**: Phase 1 searches all queries, Phase 2 follows top M results per query.

```
--count | Search results per query (default: 5)
--max   | Pages to follow per query (default: count/2)
```

```json
{
  "search_results": [
    {"title": "...", "url": "...", "snippet": "...", "query": "q1"}
  ],
  "read_pages": [
    {"success": true, "url": "...", "content": "...", "total_length": 45320,
     "offset": 0, "truncated": false,
     "sections": [{"heading": "Introduction", "content": "..."}],
     "error": null,
     "quality": {"score": 1.0, "is_ok": true, "reasons": [], "length": 45320}}
  ],
  "meta": {
    "queries": ["q1", "q2"],
    "total_search_results": 10,
    "unique_urls": 7,
    "pages_followed": 5,
    "pages_skipped": 0,
    "duration_ms": 12500
  }
}
```

**Most efficient tool for multi-topic research.** One command replaces 12+ individual calls.

## Follow (Page Reading)

### `gthings --json follow url "<url>" --max N --selector S`

Read a single page. Extracts main content via CSS selector, detects sections from h1/h2/h3.

```
--max      | Max characters to extract (default: 15000)
--selector | CSS selector for main content (default: "article,main,[role=main]")
--offset   | Character offset into full text (default: 0)
```

```json
{
  "success": true,
  "url": "https://example.com/article",
  "content": "Full extracted text...",
  "total_length": 45320,
  "offset": 0,
  "truncated": false,
  "sections": [
    {"heading": "Introduction", "content": "..."},
    {"heading": "Results", "content": "..."}
  ],
  "error": null,
  "quality": {
    "score": 1.0,
    "is_ok": true,
    "reasons": [],
    "length": 45320
  }
}
```

**Always check `quality.is_ok`** before processing content. If false, content may be a paywall, captcha, or error page.

**If `sections` is empty**, the page doesn't use semantic headings. The `content` field still has full text.

### `gthings --json follow batch "url1" "url2" "url3" --max N`

Batch page reading. Each URL gets its own tab. Use for 3+ independent URLs.

```json
[
  {"success": true, "url": "https://...", "content": "...", ...},
  {"success": true, "url": "https://...", "content": "...", ...}
]
```

## PDF Extraction

### `gthings --json pdf url "<url>"`

Extract text from a PDF at a URL.

```json
{"source": "https://arxiv.org/pdf/2405.10119", "text": "...", "length": 45230, "pages": 6}
```

**Errors handled**:
- HTTP 404: `"PDF URL '...' returned HTTP 404 Not Found. Verify the URL is correct."`
- Non-PDF content: `"URL '...' returned content type 'text/html' but does not appear to be a PDF."`
- arXiv PDFs are auto-detected. Use the abstract page URL (`arxiv.org/abs/XXXX.XXXXX`) — the /pdf/ to /abs/ rewrite is automatic.

### `gthings --json pdf file "<path>"`

Extract text from a local PDF file.

```json
{"source": "/path/to/paper.pdf", "text": "...", "length": 45230, "pages": 6}
```

PDF extraction is pure Rust — no external dependencies. Works offline.

## Browser Lifecycle

### `gthings browser start`

Start the persistent Chrome browser on port 9222. Auto-started on first use, but explicit start lets you verify.

```json
{"status": "started", "pid": 12345, "ws_url": "ws://127.0.0.1:9222/..."}
```

### `gthings browser stop`

Stop the persistent browser. Kills the Chrome process and removes the state file.

```json
{"status": "stopped", "pid": 12345}
```

Returns `{"status": "not_running"}` if no browser state found.

### `gthings browser status`

Check if the persistent browser is running.

```json
{"status": "running", "pid": 12345, "ws_url": "ws://127.0.0.1:9222/..."}
```

or:

```json
{"status": "stopped"}
```

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | Operational error (bad URL, empty results, PDF parse failure, Chrome launch failure) |
| 101+ | Panic (report as bug) |

All errors produce messages on stderr with the failing URL/query included.
