---
name: gsearch
description: "Search Google + read web pages + extract PDFs. Parallel multi-tab batch operations. arXiv PDFs auto-convert to abstract pages. Use ONLY when you need web research — not for local file operations or system administration."
compatibility: "Bun required (Chrome or Dia)"
---


##  When This Skill Activates

The user says ANY of these phrases → you MUST load this skill and use gsearch commands:
- "/gsearch", "gsearch", "/gsearch skill"
- "search for", "search the web", "research", "find papers", "look up"
- "arxiv", "google scholar", "latest papers", "find me"

### What you must do:

1. Use `gsearch batch search --count N "query1" "query2" ...` for multi-query discovery
2. Use `gsearch search "query"` for a single fact
3. Use `gsearch follow <url>` to read any full page
4. Use `gsearch batch follow <url1> <url2> ...` for multiple pages
5. Use `gsearch batch harvest --count N --max M "q1" "q2" ...` for full research pipelines
6. Use `gsearch follow --offset N --max M` for incremental reading of long pages (when content.truncated is true, request the next chunk)

### What you must NOT do:

**Do NOT use `webfetch`** for any web research task. See Trap 7 (Fetch-Tool-Fallback Trap) below for why.

### Why this matters

gsearch launches a real Chrome browser via CDP. It executes JavaScript, bypasses Cloudflare/bot-detection, renders dynamic pages, and extracts visible text from real rendered DOM. webfetch sends a raw HTTP request — no JS execution, no anti-bot evasion, returns empty shells or captcha pages. For arXiv, Google Scholar, SSRN, or any real website, gsearch follow is the only tool that works reliably.

## The Core Rule

**gsearch delegates all browser automation to the CDP skill.** gsearch handles search strategy (queries, dedup, source selection). Browser operations (page load, extraction, tabs) are handled by `scripts/browser-automation.ts` in the CDP skill. See `agents/skills/cdp/SKILL.md` for the browser automation reference.

**Under the hood:** browser-harness-js is a single-threaded REPL. Shell `&` does NOT parallelize — it queues at the server. gsearch batch commands create multiple CDP tabs within one invocation for true parallelism.

**PDFs are handled automatically.** arXiv PDFs convert to abstract page HTML. Others get Chrome viewer extraction (15,000 character cap). For academic papers or any PDF where you need the full text, skip Chrome viewer — use `gsearch pdftotext <url>` directly. It has no character cap and extracts the complete paper text. Never use batch follow for academic papers you intend to read in full — you'll only get the first 15k characters.

## Required First Pass

```bash
gsearch launch
# → {"success":true,"pid":...,"port":9222}
```

Starts isolated browser (Chrome or Dia) at `/tmp/gsearch-profile` with `--remote-debugging-port=9222`. Run once. Your real browser is never touched.

## Reference Documents

| File | Content |
|------|---------|
| `reference/commands.md` | All commands with flags, return types, error codes, environment variables |
| `reference/pdf-extraction.md` | 3-tier PDF handling (arXiv rewrite, Chrome viewer, pdftotext), URL detection logic, error reference |
| `reference/troubleshooting.md` | CDP connection failures, empty results, PDF extraction issues, environment setup, exit codes |
| `reference/ast-discovery.md` | Structured knowledge tree for web research — provenance, token efficiency, anti-hallucination |
| `cdp/reference/workspace-management.md` | `/tmp/nyx-search/` workspace, session isolation, cache TTL, artifact lifecycle, cleanup rules — shared with CDP skill |

## Architecture

gsearch is a search strategy skill. All browser automation is handled by the CDP skill.
gsearch bash commands call `cdp/scripts/browser-automation.ts` for browser operations.

See `agents/skills/cdp/SKILL.md` for browser automation reference.

## Design Rationale

**15k cap is information-theoretically calibrated.** Academic text entropy is ~1.4–1.7 bits/char. A coherent idea unit (one complete section: Methods, Results) spans 3k–12k chars. The 15k boundary captures 1–2 such units — enough for semantic coherence, bounded for aggregation precision. Beyond this, mutual information I(Source;Chunk) follows Zipf-Mandelbrot diminishing returns, and LLM "Lost in the Middle" degradation (Liu et al. 2023) reduces retrieval by 20–40%.

**Tree mapping for traceability.** Each extraction is a morphism URL → Content with labeled provenance:
```text
follow (forgetful functor)     — 15k cap, fast scan
pdftotext (faithful functor)   — full text, no cap
chunk → Tree&lt;Section&gt;        — natural transformation preserving section boundaries
```
Every content node carries `{source_url, method, offset, length}`. All transformations compose deterministically — aggregators can trace any chunk back to source through a commuting diagram of morphisms. This is a DAG of functors, not a pipeline of opaque steps.

## Playbook

1. **Launch first.** `gsearch launch`, one time. All commands auto-connect after.
2. **Use `batch harvest` for any multi-source topic.** One command = parallel search + dedup + parallel read.
3. **Give each query a different angle.** "US Fed" + "ECB policy" + "Japan Nikkei" = 3 dimensions. "S&P 500" + "Dow Jones" + "Nasdaq" = 1 dimension, 3 times.
4. **Use `batch search` when you only need URLs + snippets** — not full pages.
5. **Use `batch follow` when you already have URLs** — reads all in parallel.
6. **Use `gsearch "query"` for a single narrow fact.**
7. **Never shell-background (`&`) gsearch commands.** It queues at the REPL.
8. **Never open `chrome://inspect/`.** It does NOT enable CDP. Use `gsearch launch`.
9. **Check `_error` fields in batch results.** That's how you know what failed and why.
10. **Use `gsearch pdftotext <url>` for academic papers or full-text PDFs** — it has no 15,000-character cap and extracts the complete document. Use batch follow only when you need a preview or abstract. If Chrome viewer extraction fails on a non-academic PDF, pdftotext is also the fallback.
11. **Use gsearch for ALL web access.** When the user says "gsearch" or asks you to research, use gsearch commands for ALL web access — gsearch search, gsearch follow, gsearch batch harvest. Do NOT fall back to webfetch or other general-purpose fetch tools. The user chose gsearch for CDP-powered browser automation (JS rendering, anti-bot bypass, parallel tabs), not raw HTTP fetching.
12. **Use `--offset`/`--max` for incremental reading** — follow and batch follow now return structured output with `{content, total_length, truncated, sections}`. When `truncated` is true, request the next chunk with `--offset <current_offset + max>`.

## The Traps You Will Hit

### Trap 1: The Background-&-Wait Trap

**You think:** "I'll run 3 searches with `&` and `wait` for 3x speed."

**What happens:** The REPL processes them serially. 3 sequential calls at 5s each = 15s.

**Detect:** Results come back one-at-a-time despite `&`.

**Fix:** `gsearch batch search q1 q2 q3` — one invocation, concurrent tabs, ~5s total.

### Trap 2: The Inspect-Page Trap

**You think:** "chrome://inspect toggle enables CDP."

**What happens:** That toggle discovers *remote* targets. It does NOT start a local CDP server. Old code popped up the browser creating an infinite useless loop.

**Detect:** Dia/Chrome keeps popping up. Toggle does nothing.

**Fix:** `gsearch launch` starts `--remote-debugging-port=9222`. Never open inspect page.

### Trap 3: The Narrow-Queries Trap

**You think:** "I'll search 'US stocks', 'S&P 500', 'Dow Jones' for thorough coverage."

**What happens:** All three return the same URLs — same Reuters article 3 times.

**Detect:** Results have duplicate URLs across queries.

**Fix:**
```bash
# Bad: same dimension
gsearch batch harvest "US stocks" "S&P 500" "Dow Jones"

# Good: three independent dimensions
gsearch batch harvest "Fed rate July 2026" "ECB monetary policy" "Japan Nikkei"
```

### Trap 4: The PDF-Is-Empty Trap

**You think:** "PDF URLs will extract text like HTML pages."

**What happens:** Chrome renders PDF as canvas — `document.querySelector()` returns empty.

**Fix:** arXiv PDFs auto-convert (no action needed) — but you only get the abstract (~15k chars max). For the full paper text, use `gsearch pdftotext <url>` directly. Other PDFs get Chrome viewer extraction (also capped at 15k chars). If `_error: "pdf_textlayer_empty"`, use:
```bash
gsearch pdftotext "https://example.com/paper.pdf"
```

**Key rule for aggregators:** batch follow caps content at 15,000 characters per page. pdftotext has NO cap. If you need the full document, reach for pdftotext first, not batch follow.

### Trap 5: The Dead-Tab Trap

**You think:** "A connection error will crash the whole batch."

**What happens:** Each tab is isolated with try/catch. Dead tabs get `_error`, live tabs continue.

**Detect:** `_error: "ERR_CONNECTION_CLOSED"` in results.

**Fix:** Nothing — error isolation is built in. Check `meta.pages_skipped`.

### Trap 6: The Paywall-Teaser Trap

**You think:** "The page loaded and returned text — that is useful."

**What happens:** "Read More »" is < 80 chars of real information. The content quality gate filters these automatically.

**Detect:** `_error: "low_quality_content"`.

**Fix:** Find better sources. If too many pages are filtered, your source selection needs improvement.

### Trap 7: The Fetch-Tool-Fallback Trap

**You think:** "I need to read a paper abstract — I will use webfetch for this."

**What happens:** The user asked you to use gsearch. You used a raw HTTP fetch tool that cannot render JavaScript, bypass anti-bot, or handle dynamic pages. The content may be empty or blocked.

**Detect:** You are using `webfetch` (or a similar HTTP fetch tool) after the user said "gsearch" or after you loaded the gsearch skill.

**Fix:** Use `gsearch follow <url>` for every page you read. That is what gsearch is for — CDP-powered browser automation with JS rendering, anti-bot evasion, and parallel tab support. Never fall back to HTTP fetch tools when the user chose gsearch.

**Rationale:** webfetch sends a raw HTTP request. It cannot execute JavaScript, cannot bypass Cloudflare, and returns whatever the server sends — often an empty shell or a captcha page. gsearch follow launches a real Chrome tab, renders the page, executes JS, and extracts visible text. For arXiv abstracts, SSRN papers, Google Scholar profiles — any real website — gsearch follow is what you need.

### Trap 8: The 15k-Char-Cap Trap

**You think:** "I'll use batch follow to read this academic paper — it'll return the full text."

**What happens:** `batch follow` and `follow` cap extracted content at **15,000 characters per page**. Academic papers routinely exceed this (a typical 10-page paper is ~30,000-50,000 chars). You get the first few pages, then silence. No error, no warning — just truncated content.

**Detect:** The paper's content ends abruptly mid-section. No `_error` field is set because 15k chars passes the quality gate.

**Fix for HTML pages:** Use `gsearch follow <url> --offset 15000 --max 15000` to get the next chunk. The `truncated` field in the output tells you if more content is available. Build the full document tree by iterating:
```bash
# Chunk 1: offset=0, max=15000
gsearch follow <url> --offset 0 --max 15000 --pretty
# Chunk 2: offset=15000, max=15000 (if truncated=true)
gsearch follow <url> --offset 15000 --max 15000 --pretty
```

**Fix for PDFs:** Use `gsearch pdftotext "https://arxiv.org/pdf/xxxx.xxxxx.pdf"` instead. It has no character cap and extracts the full document:

```bash
# BAD — truncated at 15k chars
gsearch batch follow "https://arxiv.org/pdf/2401.12345.pdf"

# GOOD — full paper text, no cap
gsearch pdftotext "https://arxiv.org/pdf/2401.12345.pdf"
```

**Rule of thumb:**
- Need a preview / abstract? → batch follow (15k is fine)
- Need the full academic paper? → pdftotext (no cap)
- Need an HTML page beyond 15k? → follow with --offset pagination (check `truncated` field)

## Decision Tree

```
Topic to research?
├─ Yes → One fact or broad topic?
│        ├─ One fact → gsearch "query" → pick URL → gsearch follow <url>
│        └─ Broad → gsearch batch harvest --count 5 --max 3 "q1" "q2" "q3"
├─ No → Have URLs? → gsearch batch follow url1 url2 url3
├─ No → Need discovery? → gsearch batch search --count 5 "q1" "q2"
└─ Already read? Use --offset for next chunk:
         gsearch follow <url> --offset 15000 --max 15000

URL is a PDF?
├─ Academic paper where you need full text → gsearch pdftotext <url> (no 15k cap)
├─ arXiv PDF (abstract only needed) → auto-converted to abstract HTML (~15k chars)
├─ Other PDF → Chrome viewer extracts (15k cap). If _error → gsearch pdftotext <url>
└─ Not PDF → normal extraction (15k cap, use --offset to paginate)
```

## Rationalization Table

| You think | Do instead |
|-----------|------------|
| "I'll run 3 gsearch with `&` for speed" | Use `batch search` — one invocation, true parallelism |
| "chrome://inspect enables CDP" | `gsearch launch` — auto-starts CDP |
| "3 similar queries give thorough coverage" | Each query must be a different dimension |
| "PDFs extract like HTML" | arXiv PDFs auto-convert. Others → `gsearch pdftotext` |
| "I'll use batch follow for this academic paper" | Use `gsearch pdftotext` — batch follow caps at 15k chars, pdftotext has no cap and returns the full paper |
| "A dead URL crashes the whole batch" | No — tabs are isolated. Failed tabs get `_error` |
| "I will use webfetch to read this paper" | Use `gsearch follow <url>` — the user chose gsearch for CDP-powered browser automation. webfetch cannot render JS or bypass anti-bot. |
| "I hit the 15k cap mid-article" | Use `gsearch follow <url> --offset 15000 --max 15000` to get the next chunk. Check `truncated` field to know if more exists. |

## Checklist

1. Did `gsearch launch` succeed? (`{"success":true,"port":9222}`)
2. Did you use `batch harvest` for multi-source research? (If not, it's slower.)
3. Are queries covering different angles? (Overlapping queries waste time.)
4. Did you avoid shell `&`? (It queues — doesn't parallelize.)
5. Did you check `_error` fields? (Tells you which pages failed.)
6. Did arXiv PDFs return content? (They should auto-convert.)
7. For PDF `_error` → did you try `gsearch pdftotext`?
8. For academic papers where you need full text → did you use `gsearch pdftotext` instead of batch follow? (batch follow caps at 15k chars, pdftotext has no cap)
9. Is `pages_read > pages_skipped`? (If not, find better sources.)
10. Did you use gsearch commands (search/follow/batch) for ALL web access, not webfetch? (If you used webfetch, the user did not ask for it — redo with gsearch.)

## Environment Variables

| Variable | Default | Override when |
|----------|---------|---------------|
| `GSEARCH_CDP_PORT` | 9222 | Port already in use |
| `GSEARCH_PROFILE_DIR` | `/tmp/gsearch-profile` | Custom profile location |
| `GSEARCH_TOKEN_DIR` | `/tmp/gsearch-tokens` | Token storage |
| `CHROME_PATH` | auto-detect | Force a specific browser |
