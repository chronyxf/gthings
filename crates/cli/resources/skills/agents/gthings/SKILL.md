---
name: gthings
description: "Browser automation and web research CLI — search, follow, extract, PDF extraction, batch search, quality gate"
---

# Skill: gthings

Browser automation and web research toolkit. Native Rust binary — single static binary that reuses existing Chrome/Dia browser via CDP.

## When This Skill Activates

The user says any of these:
- "search" "find" "look up" "research"
- "gthings"
- "web research" "browser research"
- "pdf" "extract"

## Core Rules

1. **Use `--json`** for structured output.
2. **Prefer `batch`** for multi-query search.
3. **Browser is already running** — no launch needed. Use `gthings status` to verify.
4. **Check quality score** in PDF extraction output.

## Commands

### `gthings search <query> [--count=N]`

Single Google search via CDP browser. Returns numbered results with title, URL, snippet.

```
--count  Number of results (default: 10)
--json   Output as JSON array [{title, url, snippet}]
```

### `gthings follow <url> [--max-chars=N]`

Read a page via CDP browser. Extracts visible text content.

```
--max-chars  Max characters to extract (default: 15000)
--json       Output as JSON {title, url, content}
```

### `gthings batch <queries...> [--count=N] [--follow] [--max-chars=N]`

Multi-query search. Each query opens a separate CDP tab.

```
--count      Results per query (default: 10)
--follow     Also follow/read top result per query
--max-chars  Max chars per follow (default: 15000)
--json       Output as JSON array of arrays
```

### `gthings status`

Check CDP browser connection. Returns browser name and WebSocket URL.

### `gthings extract <url> [--max-chars=N]`

Auto-detect and extract content from any URL. Detects PDF, GitHub source, arXiv paper, or web page automatically. Uses pdftotext for PDFs.

```
--max-chars  Max characters to extract (default: 15000)
--json       Output as JSON
```

### `gthings pdf url <url> [--json]`

Extract text from a PDF at URL using pdftotext. arXiv PDFs supported. Quality score included.

```json
{"quality": {"score": 0.9, "is_ok": true}, "pages": 8, "body": {"Pdf": {"text": "..."}}}
```

### `gthings pdf file <path> [--json]`

Extract text from a local PDF file via pdftotext.

## PDF Extraction Notes

- Uses `pdftotext` (poppler-utils) — install via `brew install poppler`
- Quality score: 0.90/1.0 for clean readable text
- Output includes full paper text with sections, references, footnotes
- No 15k character cap — extracts complete document

## Quality Score

| Score | Meaning |
|-------|---------|
| 0.90 | Clean, readable text with good structure |
| 0.80 | Readable with minor artifacts |
| 0.30 | Low quality — artifacts detected, retry with pdftotext |
| 0.00 | Extraction failed |

## Error Handling

| Problem | Fix |
|---------|-----|
| "Cannot drop a runtime" on extract | Use `gthings pdf url` or `gthings follow` instead |
| PDF text empty | Ensure `pdftotext` is installed (`brew install poppler`) |
| Browser not found | Browser (Chrome/Dia) must be running with `--remote-debugging-port=9222` |
| Empty search results | Try different wording |
