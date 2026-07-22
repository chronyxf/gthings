# Commands reference

All commands auto-connect to the running browser. Only `gsearch launch` needs an explicit call — everything else detects the browser automatically.

## `gsearch launch`

Starts an isolated Chromium instance at `/tmp/gsearch-profile` with `--remote-debugging-port=9222`. No cookies, no user profile. Your real browser is never touched.

```bash
gsearch launch
# → {"success":true,"pid":47313,"port":9222}
```

If a browser is already running with CDP, this detects and reuses it (`"reused":true`). Run once per session.

## `gsearch batch search [--count N] query1 query2 ...`

Runs N queries in parallel CDP tabs. One JS invocation, multiple tabs. Results are deduplicated by URL and sorted by snippet length.

```bash
gsearch batch search --count 5 "Fed rate" "ECB policy" "BOJ decision"
```

`--count N` — max results per query (default 5). Each tab loads a Google search result page and extracts `{title, url, snippet}` from attribute-based selectors (`a[href]` filtered by hostname, `data-hveid` container) — resilient to Google's class name rotation.

Returns:
```json
{
  "results": [
    {"title":"...","url":"...","snippet":"...","_query":"Fed rate"}
  ],
  "meta": {"queries":["Fed rate","ECB policy"],"total_unique":8,"total_raw":15}
}
```

The `_query` field tags each result with its source query so you can trace provenance. Results from different queries that share the same URL are merged (first occurrence wins).

## `gsearch batch follow [--selector S] [--offset N] [--max M] [--pretty] url1 url2 ...`

Reads N URLs in parallel. Each URL gets its own CDP tab. Pages load concurrently in the browser.

```bash
gsearch batch follow \
  "https://reuters.com/article1" \
  "https://arxiv.org/pdf/2401.12345.pdf"
```

`--selector S` — CSS selector for text extraction (default `article, main, [role=main]`). Falls back to `document.body.innerText` if the selector returns nothing.

**PDF handling is automatic for batch mode only.** `gsearch batch follow` uses `batchFollowCode` (templates.ts:201) which has specific PDF handling: arXiv URLs (`arxiv.org/pdf/...`) are rewritten to `arxiv.org/abs/...` before tab creation — the abstract page is HTML. Other PDFs are loaded in Chrome's PDF viewer and text extraction is attempted after 3s.

`gsearch follow` (single URL) uses `followCode` (templates.ts:149) which has **no PDF-specific handling** — it uses the generic extraction function. For PDFs in single-follow mode, use `gsearch pdftotext <url>` instead.

| Flag | Default | Description |
|------|---------|-------------|
| `--offset N` | 0 | Character offset to start reading from. Start at 0, then increment by --max to paginate |
| `--max M` | 15000 | Max characters to return per page. Set to -1 for unlimited (caution: large pages) |
| `--pretty` | off | Pretty-print JSON output |
| `--settle MS` | 0 | Extra settle time in ms after page load for JS-rendered content (SPAs) |
| `--wait M` | `networkIdle` | Lifecycle event to wait for: `networkIdle` (default), `almostIdle`, or `load` |

**New output format** (structured content tree):
```json
{
  "url": "https://...",
  "content": "extracted text from offset to offset+max...",
  "total_length": 45320,
  "returned_length": 15000,
  "offset": 0,
  "truncated": true,
  "sections": [
    {"heading": "Introduction", "level": 1, "offset": 0},
    {"heading": "Methods", "level": 2, "offset": 3200}
  ]
}
```

The `truncated` field tells you whether more content is available. The `sections` array provides a table of contents extracted from h1/h2/h3 elements. Each section has its character offset in the full document, enabling aggregators to build a navigation tree.

Incremental reading example:
```bash
# First chunk
gsearch batch follow "https://long-article.com" --offset 0 --max 15000

# If truncated is true, get next chunk
gsearch batch follow "https://long-article.com" --offset 15000 --max 15000

# Continue until truncated is false
```

Returns `[{url, content, total_length, returned_length, offset, truncated, sections, _error?}]`. Content is capped at 15000 characters per page. Use `--max -1` for unlimited content. Use `--pretty` to format JSON with indentation.

| `_error` | Meaning | What to do |
|----------|---------|------------|
| `pdf_textlayer_empty` | Chrome's PDF viewer found no text | Try `gsearch pdftotext <url>` |
| `low_quality_content` | Under 80 chars or matches error page pattern | Find a different source |
| `ERR_CONNECTION_*` | DNS / connection / timeout failure | Retry later or skip |
| `pdf_extraction_failed: ...` | JS threw during PDF extraction | Try `gsearch pdftotext <url>` |

## `gsearch batch harvest [--count N] [--max M] query1 query2 ...`

Full research pipeline in one invocation. Phase 1 searches all queries in parallel. Phase 2 deduplicates, ranks by snippet length, picks top M URLs, and reads them all in parallel.

```bash
gsearch batch harvest --count 5 --max 3 \
  "Fed interest rate July 2026" \
  "ECB monetary policy outlook"
```

| Flag | Default | Description |
|------|---------|-------------|
| `--count N` | 5 | Max results per search query |
| `--max M` | 5 | Max URLs to read in phase 2 |
| `--maxN N` | — | Max results per topic for batch harvest |
| `--topics` | off | Explicit topic flag for batch harvest |

Returns:
```json
{
  "search_results": [
    {"title":"...","url":"...","snippet":"...","_query":"Fed..."}
  ],
  "read_pages": [
    {"url":"...","title":"...","query":"...","content":"...","_error?":"..."}
  ],
  "meta": {
    "queries": ["q1","q2"],
    "total_search_results": 10,
    "unique_urls": 8,
    "pages_read": 2,
    "pages_skipped": 1
  }
}
```

`pages_read` counts URLs that returned valid content. `pages_skipped` counts PDFs, connection errors, and low-quality content. If `pages_skipped` is high, your queries may be returning too many paywalled or PDF sources. Use `--pretty` with `gsearch follow` and `gsearch batch follow` to format JSON output with indentation for human readability.

## `gsearch pdftotext <url>`

Downloads a PDF via `curl` and extracts text using a TypeScript parser at `scripts/pdf-extract.ts`. The parser uses Node.js built-in `zlib` to decompress FlateDecode streams — zero external dependencies.

```bash
gsearch pdftotext "https://example.com/paper.pdf"
# → {"url":"...","content":"extracted text..."}
```

Use this when:
- A PDF in `batch follow` returned `_error: "pdf_textlayer_empty"`
- You have a PDF URL and want the full text

Limitations:
- Scanned/image-only PDFs have no text layer — extraction returns empty
- Encrypted PDFs are not supported
- PDFs with non-standard compression may fail

## `gsearch "query" [--count N]`

Single-query Google search. Returns results as a JSON array.

```bash
gsearch --count 5 "Fed interest rate"
# → [{"title":"...","url":"...","snippet":"..."}]
```

`--count N` — max results (default 10). Use this for narrow lookups where you only need one or two results.

## `gsearch follow <url> [--selector S] [--offset N] [--max M] [--pretty] [--raw]`

Reads a single page. Options:

| Flag | Default | Description |
|------|---------|-------------|
| `--selector S` | `article, main, [role=main]` | CSS selector for text |
| `--raw` | off | Output raw text without JSON wrapper |
| `--offset N` | 0 | Character offset to start reading from |
| `--max M` | 15000 | Max characters to return. Set to -1 for unlimited |
| `--pretty` | off | Pretty-print JSON output |
| `--settle MS` | 0 | Extra settle time in ms after page load for JS-rendered content (SPAs) |
| `--wait M` | `networkIdle` | Lifecycle event to wait for: `networkIdle` (default), `almostIdle`, or `load` |
| `--json-url` | off | Treat the URL as a JSON source |

## `gsearch screenshot <url> [--output FILE]`

Full-page PNG screenshot. Saved to disk.

## `gsearch scrape <url> [--selector S] [--attr A] [--list]`

Extracts structured data using CSS selectors. Useful for scraping lists, tables, or specific attributes from a page.

```bash
gsearch scrape "https://example.com" --selector "h1" --list
# → ["Title 1","Title 2"]
gsearch scrape "https://example.com" --selector "a" --attr "href" --list
# → ["/page1","/page2"]
```

## Environment variables

| Variable | Default | When to override |
|----------|---------|------------------|
| `GSEARCH_CDP_PORT` | 9222 | Port is already in use by another process |
| `GSEARCH_PROFILE_DIR` | `/tmp/gsearch-profile` | Want to use a specific browser profile |
| `GSEARCH_TOKEN_DIR` | `/tmp/gsearch-tokens` | Need token storage in a different location |
| `CHROME_PATH` | auto-detect | Must use a specific browser binary (e.g., `/path/to/chrome`) |
