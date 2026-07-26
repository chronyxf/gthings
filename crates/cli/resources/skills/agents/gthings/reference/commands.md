# gthings CLI Command Reference

All commands support `--json` for structured JSON output.

## search

```
gthings search <query> [--count N] [--json]
```

Single Google SERP search via CDP browser.

**Output** (`SearchResult[]`):
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

## follow

```
gthings follow <url> [--max-chars N] [--offset N] [--json]
```

Single page content via CDP browser (`document.body.innerText`).

**Output** (`FollowResult`):
```json
{
  "url": "https://example.com/page",
  "title": "Page Title",
  "content": "Full visible text content...",
  "error": "",
  "provenance": { "source_url": "...", "method": "Follow", "agent": "gthings/0.6.0", "accessed_at": "...", "duration_ms": 500 },
  "pagination": { "offset": 0, "returned_len": 15000, "total_len": 45320, "truncated": true, "continuation_token": "..." }
}
```

If `pagination.truncated == true`, fetch next chunk with `--offset N`.

## extract

```
gthings extract <url> [--max-chars N] [--offset N] [--json]
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

## batch

```
gthings batch <q1> [<q2> ...] [--count N] [--follow] [--max-chars N] [--json]
```

Multi-query search. Returns `SearchResult[][]` (one array per query). With `--follow`, follows top result per query.

## harvest

```
gthings harvest <q1> [<q2> ...] [--follow-top N] [--max-chars N] [--dedup url] [--rank composite] [--json]
```

Full research pipeline. One command replaces search + dedup + rank + select + follow + quality.

**Pipeline**: parallel search (JoinSet) → dedup (canonical URL normalization) → rank (composite or other strategy) → select (per-query minimum, per-host cap max 2, junk URL filter) → parallel follow (JoinSet, 30s timeout) → quality scoring → summary.

**Output**:
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

**BodyStatus values**: `ok` | `snippet_only` | `extract_failed` | `pdf_unextracted` | `chrome_or_empty`

**Rank strategies**:
- `serp_order` — Google's original order, interleaved round-robin across queries
- `domain_authority` — Descending by domain authority score
- `snippet_length` — Descending by SERP snippet length
- `composite` (default) — `0.5 * authority + 0.3 * norm_snippet + 0.2 * diversity_bonus`

## pdf

```
gthings pdf url <url> [--json]
gthings pdf file <path> [--json]
```

PDF extraction via pdftotext. Requires poppler. Output includes full text, pages, quality score.

```json
{
  "url": "https://arxiv.org/pdf/2405.10119",
  "quality": { "score": 0.90, "is_ok": true, "reasons": [], "entropy_bits_per_char": 4.5 },
  "pages": 8,
  "body": { "Pdf": { "text": "Full PDF text...", "pages": 8, "has_toc": true } }
}
```

## status

```
gthings status [--json]
```

Browser connection check.

```json
{ "status": "running", "pid": 12345, "ws_url": "ws://127.0.0.1:9222/..." }
```

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | Operational error (bad URL, empty results, PDF parse failure) |
| 101+ | Panic (report as bug) |
