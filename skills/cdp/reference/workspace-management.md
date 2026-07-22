# Workspace Management — `/tmp/nyx-search-cache/`

## Purpose

`/tmp/nyx-search-cache/` is the canonical cache directory for all gsearch and CDP browser automation operations. Every fetched page, extracted PDF text, and API response is cached here to avoid redundant network requests across agent invocations.

## Directory Structure

Flat file structure — no subdirectories:

```
/tmp/nyx-search-cache/
├── <sha256-hex>.json      # Cached page content (cache.ts)
├── <sha256-hex>            # Cached PDF text (pdf.sh, pdf-extract.ts)
└── ...
```

Cache keys are **SHA-256** (or SHA-1 in some contexts) hex digests of the source URL. The cache is a single flat directory — every file sits at the root with no nested subdirectories.

## Cache Behavior

| Aspect | Detail |
|--------|--------|
| **Location** | `/tmp/nyx-search-cache/` — defined as `CACHE_DIR` in `cache.ts:29` |
| **Key generation** | SHA-256 hex of the URL (`cache.ts`, `pdf-extract.ts`). Some shell scripts use SHA-256 (`pdf.sh`) |
| **File format** | JSON for HTML pages (`{url, content, cachedAt}`), raw text for PDFs |
| **Enforcement** | **Unconditional** — no `--cache` flag. Cache reads/writes happen in `browser-automation.ts`, `pdf.sh`, and `pdf-extract.ts` on every invocation |
| **Environment** | No env vars control the cache path. The path is hardcoded in `cache.ts` and shell scripts |

## Writes

Three code paths write to the cache:

1. **`cache.ts`** (`/nyx/agents/skills/cdp/scripts/cache.ts`) — Used by `browser-automation.ts`. On fetch: computes SHA-256 of the URL, checks for cached file, reads if present; on response: writes `{url, content, cachedAt, contentLength}` JSON. Creates the directory via `mkdirSync` with `recursive: true` if absent.

2. **`pdf.sh`** (`/nyx/agents/skills/gsearch/lib/pdf.sh`) — Downloads PDF via `curl`, extracts text, caches at `/tmp/nyx-search-cache/$sha1` (SHA-1 of URL string). Creates directory with `mkdir -p` on first write.

3. **`pdf-extract.ts`** (`/nyx/agents/skills/gsearch/scripts/pdf-extract.ts`) — TypeScript-based PDF parser. Computes SHA-256 of the URL, caches extracted text at `/tmp/nyx-search-cache/$sha256`. Creates directory with `mkdirSync` if absent.

## Reads

The cache is consulted before any fetch. If a file exists for the URL's key, the cached content is returned immediately — no network request is made. Cache reads are **best-effort**: if the file is missing or unreadable, the code falls through to fetch the live URL.

## Cleanup

Since the cache lives under `/tmp`, it is subject to the OS temp directory cleanup policy. On most systems, files older than 10 days are automatically purged. For manual cleanup:

```bash
# Check disk usage
du -sh /tmp/nyx-search-cache/

# Count cached entries
ls /tmp/nyx-search-cache/ | wc -l

# Clear all cache (safe — regenerated on next fetch)
rm -f /tmp/nyx-search-cache/*

# Ensure directory exists for future writes
mkdir -p /tmp/nyx-search-cache
```

## Safety Rules

1. **Cache is always regeneratable** — cached content can be re-fetched. Safe to delete at any time.
2. **Never delete the directory itself** — if you do, recreate it with `mkdir -p /tmp/nyx-search-cache/`.
3. **No lock files** — concurrent writes to the same cache key may race. The last writer wins. Data integrity is not guaranteed under concurrent access but is sufficient for idempotent fetch-cache workflows.
