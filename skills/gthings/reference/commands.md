# gthings CLI Command Reference

Every command supports `--json` (global, structured JSON output) and `--trace <file>` (telemetry).

## Browser Management

### `gthings browser start --port <port>`

Start the daemon. Must be running before any browser operations.

```
--port     | Browser debugging port (default: 9222)
```

```json
{"ok":true,"pid":47313,"port":9222}
```

### `gthings browser status`

Check if daemon is running and connected.

```json
{"ok":true,"pid":47313,"port":9222,"connected":true,"uptime_secs":120}
```

### `gthings browser stop`

Gracefully stop the daemon.

```json
{"ok":true,"stopped":true}
```

### `gthings browser call <method> <params>`

Raw CDP method call. `params` is JSON string.

```
gthings --json browser call "Browser.getVersion" "{}"
→ {"product":"Chrome/150.0.7871.129","protocol":"1.3",...}
```

### `gthings browser eval <expression>`

Evaluate JavaScript in the browser context.

```
gthings --json browser eval "1+1"
→ {"value":2}
```

### `gthings browser navigate <url>`

Navigate the current tab.

```
gthings --json browser navigate "https://example.com"
→ {"ok":true}
```

### `gthings browser logs [--follow]`

View daemon logs.

```
gthings browser logs
```

### `gthings browser wait <method> <session> [--timeout 30000]`

Wait for a CDP event. Primarily for debugging.

---

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

**This is the most efficient tool for multi-topic research.** One command replaces 12+ individual calls.

---

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

---

## Screenshot

### `gthings screenshot "<url>" --output FILE`

Capture a PNG screenshot.

```
--output | Output file path (default: screenshot.png)
```

Returns: Writes PNG to file. Or with `--json`:

### `gthings --json screenshot "<url>" --json`

```json
{"data": "<base64-encoded-png>", "format": "png", "size": 49859}
```

Use `--json` for vision-capable AI agents that consume base64 images.

---

## Scrape

### `gthings --json scrape "<url>" --selector S --attribute A`

Extract specific elements by CSS selector.

```
--selector  | CSS selector (default: "body")
--attribute | Attribute to extract (default: innerText)
```

```json
["Example Domain", "Learn more"]
```

Returns array of strings. Each element is the innerText (or attribute) of matched nodes.

---

## PDF Extraction

### `gthings --json pdf url "<url>"`

Extract text from a PDF at a URL.

```
Expected URL: A direct PDF file URL (should end in .pdf, or be an arXiv abs page)
```

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

---

## Daemon Management (alternative to CLI)

```bash
# Start
gthings browser start --port 9222

# Check
gthings --json browser status

# Raw CDP
gthings --json browser call "Target.createTarget" '{"url":"about:blank"}'
gthings --json browser eval "document.title"

# Navigate
gthings --json browser navigate "https://example.com"

# Wait for event (debugging)
gthings --json browser wait "Page.loadEventFired" "session-id" --timeout 10000

# View logs
gthings browser logs

# Stop
gthings browser stop
```

---

## Telemetry (--trace)

### `gthings --trace /tmp/run.jsonl --json search query "topic" --count 5`

Appends one JSONL line per command:

```json
{"ts":"1784705060.276456000","session":"ses_18c48bcbea5476e8","tool":"search","args":{"count":5,"query":"topic"},"duration_ms":2340,"exit":0}
```

Fields: `ts` (unix.nanos), `session` (per-process UUID), `tool`, `args`, `duration_ms`, `exit`.

Multiple invocations append to the same file. No overhead when `--trace` is absent.

---

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | Operational error (daemon down, bad URL, empty results, PDF parse failure) |
| 101+ | Panic (report as bug) |

All errors produce messages on stderr with the failing URL/query included.
