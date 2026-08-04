# gthings CLI Command Reference

All commands support `--output json` (or `--json` for backward compat) for structured JSON output.

## Universal Flags

| Flag | Description |
|------|-------------|
| `-o, --output <FORMAT>` | Output format: text, json, nd-json (default: text) |
| `-q, --query <DOT>` | Custom dot-notation filter on JSON output (e.g. `.data`, `.[].url`, `.results[].snippet` — not full JMESPath) |
| `--cdp-port <PORT>` | CDP port (default: 9222, env: GTHINGS_CDP_PORT) |
| `--cdp-url <URL>` | CDP WebSocket URL (overrides port) |
| `--timeout <SECS>` | Timeout for CDP/extraction (default: 30) |
| `-v` | Verbose (-v -v debug, -v -v -v trace) |
| `-q, --quiet` | Suppress non-error output |
| `--json` | Backward-compat alias for `--output json` |

All JSON output follows the envelope:
```json
{
  "status": "ok" | "error",
  "data": <command-specific result>,
  "error": {
    "code": "ERROR_CODE",
    "detail": "human-readable detail",
    "hint": "recovery hint"
  }
}
```

---

## search

```
gthings search <queries...> [--count N] [--strategy simple|parallel|harvest]
    [--extract-results] [--max-chars N] [--dedup STR] [--rank STR]
    [--follow-top N] [--warn-tabs N]
```

Google SERP search via CDP browser with strategy-based processing.

**Strategies:**
- `simple` (default): Single-query search, returns `{"results": [...], "query": "..."}`
- `parallel`: Multi-query parallel search, returns `{"results": [...]}` (one `{"ok": results}` or `{"error": ...}` entry per query)
- `--strategy harvest`: Full research pipeline — search → dedup → rank → select → follow → quality score → summary

**Output** (simple — `SearchResult[]`):
```json
[
  {
    "title": "Page Title",
    "url": "https://example.com/page",
    "snippet": "SERP description text...",
    "position": 1,
    "domain_authority": 0.85,
    "provenance": {
      "source_url": "https://google.com/search?q=...",
      "method": "Search",
      "agent": "gthings/0.6.0",
      "accessed_at": "2026-07-26T12:00:00Z",
      "duration_ms": 1234
    }
  }
]
```

**Output** (harvest — `{ results: HarvestedResult[], summary: HarvestRunSummary }`):
```json
{
  "results": [
    {
      "search_result": { "title": "...", "url": "...", "snippet": "...", "position": 1, "domain_authority": 0.9 },
      "followed_content": "Full page body text... or null",
      "body_status": "ok",
      "url_canonical": "https://en.wikipedia.org/wiki/entropy",
      "query": "information theory entropy",
      "quality": { "score": 0.95, "is_ok": true, "reasons": [], "entropy_bits_per_char": 4.2 },
      "sections": [{"heading": "Introduction", "content": "..."}],
      "provenance": { "source_url": "https://google.com/search?q=...", "method": "Follow", ... }
    }
  ],
  "summary": {
    "total_queries": 3,
    "total_results": 8,
    "unique_sources_followed": 5,
    "coverage_by_query": {
      "query 1": { "total_hits": 1, "followed_ok": 1, "followed_failed": 0 },
      "query 2": { "total_hits": 4, "followed_ok": 0, "followed_failed": 4 }
    },
    "warnings": ["no_body_for_query:query 2"]
  }
}
```

**Flags:**

| Flag | Default | Description |
|------|---------|-------------|
| `--count` | 5 | Results per query |
| `--strategy` | simple | simple, parallel, or harvest |
| `--extract-results` | false | Extract content from result URLs (parallel/harvest) |
| `--max-chars` | 40000 | Max chars per extracted page |
| `--dedup` | url | Dedup strategy |
| `--rank` | composite | serp, authority, snippet, composite |
| `--follow-top` | 8 | Max URLs to follow (harvest) |
| `--warn-tabs` | 20 | Warn when tabs exceed threshold (harvest) |

**BodyStatus values:** `ok` | `snippet_only` | `extract_failed` | `pdf_unextracted` | `chrome_or_empty`

**Rank strategies:**
- `serp` — Google's original order, interleaved round-robin across queries
- `authority` — Descending by domain authority score
- `snippet` — Descending by SERP snippet length
- `composite` (default) — `0.5 * authority + 0.3 * norm_snippet + 0.2 * diversity_bonus`

---

## extract

```
gthings extract <url> [--max-chars N] [--offset N]
```

HTTP-based extraction with auto-detection:
- Web URLs → Readability parser
- PDF URLs → pdftotext
- arXiv URLs → /pdf/ path rewrite + PDF extraction
- GitHub URLs → raw content fetch

**Output** (`Article`):
```json
{
  "url": "https://example.com/article",
  "title": "Article Title",
  "source": { "author": "Author Name", "site_name": "Site", "domain_authority": 0.85 },
  "extraction": { "method": "Readability", "confidence": 0.95, "accessed_at": "2026-07-26T12:00:00Z", "duration_ms": 800 },
  "body": {
    "Article": {
      "sections": [{"heading": "Introduction", "depth": 1, "content": "Section text..."}],
      "full_text": "Complete article text...",
      "total_length": 12000
    }
  },
  "quality": { "score": 0.95, "is_ok": true, "reasons": [], "entropy_bits_per_char": 4.2 }
}
```

---

## ax

```
gthings ax <url> [--max-nodes N]
```

Fetch compressed accessibility tree for a URL via CDP. Returns AX tree nodes for structured page analysis without full text render.

**Flags:**

| Flag | Default | Description |
|------|---------|-------------|
| `--max-nodes` | 500 | Max nodes in output (0 = unlimited) |

**Output** (compressed AX tree):
```json
{
  "url": "https://example.com",
  "node_count": 120,
  "truncated": false,
  "nodes": [
    { "node_id": 1, "role": "heading", "name": "Introduction", "value": "", "children": [2, 3] },
    { "node_id": 2, "role": "text", "name": "", "value": "Some text content...", "children": [] }
  ]
}
```

---

## pdf-url

```
gthings pdf-url <url> [--max-chars N] [--offset N]
```

PDF extraction from URL via pdftotext. Requires poppler (`brew install poppler`).

**Output:**
```json
{
  "url": "https://arxiv.org/pdf/2405.10119",
  "quality": { "score": 0.90, "is_ok": true, "reasons": [], "entropy_bits_per_char": 4.5 },
  "pages": 8,
  "body": { "Pdf": { "text": "Full PDF text...", "pages": 8, "has_toc": true } }
}
```

---

## pdf-file

```
gthings pdf-file <path> [--max-chars N] [--offset N]
```

PDF extraction from local file via pdftotext. Same output schema as pdf-url (uses `path` instead of `url`).

---

## status

```
gthings status
```

Browser connection check.

```json
{ "status": "running", "ws_url": "ws://127.0.0.1:9222/...", "browser": "Chrome", "version": "..." }
```

When no browser is detected, status returns `{ "status": "stopped" }`.

---

## update

```
gthings update
```

Update gthings to the latest version.

```json
{ "status": "ok", "version": "0.7.0" }
```

---

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | Operational error (bad URL, empty results, PDF parse failure) |
| 2 | Timeout (command exceeded its time budget) |
| 101+ | Panic (report as bug) |
