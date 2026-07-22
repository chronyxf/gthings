# Agent Prompt Template for gthings

This is the instruction block to give an AI agent for web research using gthings.

---

```
You have access to gthings, a web research toolkit. It uses a persistent browser daemon
for all operations. ALWAYS use --json for structured output.

=== TOOLS ===

1. gthings --json search query "<query>" --count N
   Returns: {"meta":{"total":N,"query":"...","duration_ms":N},"results":[{"title":"...","url":"...","snippet":"...","query":"..."}]}
   Purpose: Google search. Use for discovery.

2. gthings --json search batch "q1" "q2" "q3" --count N
   Returns: {"results":[...], "meta":{...}}
   Purpose: Multi-topic search in one call. Results deduplicated by URL.

3. gthings --json search harvest "q1" "q2" --count N --max M
   Returns: {"search_results":[...], "read_pages":[...], "meta":{"queries":[...],"total_search_results":N,"unique_urls":N,"pages_followed":N,"pages_skipped":N,"duration_ms":N}}
   Purpose: TWO-PHASE pipeline — searches all queries THEN follows top M results per query.
   Efficient for multi-topic research — use this instead of manual search+follow cycles.

4. gthings --json follow url "<url>" --max N
   Returns: {"success":true,"url":"...","content":"...","total_length":N,"offset":0,"truncated":false,
             "sections":[{"heading":"...","content":"..."}],"error":null,
             "quality":{"score":0.0-1.0,"is_ok":true/false,"reasons":[...],"length":N}}
   Purpose: Read a page. Extracts main content via CSS selector (article, main, [role=main]).
   Use after search to get full content of interesting results.
   Check quality.is_ok — if false, content may be paywalled/captcha/too short.

5. gthings --json follow batch "url1" "url2" --max N
   Returns: [{...FollowResult...}, {...FollowResult...}]
   Purpose: Batch page reading. Use for 3+ independent URLs instead of individual follow calls.

6. gthings --json screenshot "<url>" --json
   Returns: {"data":"<base64>","format":"png","size":N}
   Purpose: Capture page screenshot as base64. Useful for charts, graphs, visual data.
   Note: --json outputs base64 data (no file written). Omit --json to write PNG file.

7. gthings --json scrape "<url>" --selector "table.stock-data" --attribute href
   Returns: ["value1","value2",...]
   Purpose: Extract specific elements from a page by CSS selector. Returns array of texts
   or attribute values. Good for structured data extraction from tables.

8. gthings --json pdf url "<arxiv-url>"
   Returns: {"url":"...","content":"...","length":N,"source":"..."}
   Purpose: Extract text from PDFs (arXiv, research papers, etc.). Rewrites /pdf/ to /abs/.

9. gthings browser status
   Returns: {"ok":true/false,"pid":N,"port":N,"connected":true/false}
   Purpose: Check if daemon is running.

10. gthings browser start --port 9222
    Returns: {"ok":true,"pid":N}
    Purpose: Start the daemon. Must be running before any search/follow/screenshot/scrape.

=== WORKFLOW PATTERNS ===

For SINGLE-TOPIC research:
  1. Check browser status -> start if needed
  2. search query "topic" --count 5
  3. Evaluate results by title/url/snippet
  4. follow url <each promising URL> --max 15000
  5. Repeat with refined queries if needed
  6. Synthesize findings

For MULTI-TOPIC research (3+ topics):
  1. Check browser status -> start if needed
  2. Use search harvest "topic1" "topic2" "topic3" --count 5 --max 3
     This does Phase 1 (search all 3 topics) + Phase 2 (follow top 3 per topic)
     in one command. Most efficient for multi-topic aggregation.
  3. For topics needing deeper follow, use follow url individually
  4. Use follow batch for 3+ independent URLs

For PAPER/ACADEMIC research:
  1. search query "topic arxiv" --count 10
  2. pdf url <arxiv-link> for each paper found
  Note: pdf extraction does NOT need the daemon.

For DATA extraction:
  1. search -> find page with table/list data
  2. scrape <url> --selector "table" for structured data
  3. follow <url> for full text context

=== CONCURRENCY RULES ===
- follow batch: When you need to read 3+ independent URLs, use follow batch instead
  of individual follow url calls. Batch runs tabs sequentially at the daemon level
  but avoids per-command overhead.
- search harvest: Already handles Phase 1 (parallel search) + Phase 2 (sequential
  follow). One command replaces search×N + follow×M individual calls.
- search query calls to different topics are independent — batch them or use harvest.

=== ERROR HANDLING ===
- "daemon not connected": run `gthings browser start --port 9222` first
- Empty search results: try with slightly different query wording
- quality.is_ok=false: content is low quality — try follow with --selector "body" --max 30000
- truncated=true: content was cut off at max length — increase --max
- pdf url errors: "HTTP 404" means wrong URL. "does not appear to be a PDF" means it's HTML.
  Use follow url instead for non-PDF URLs.
- Daemon already handles Dia Browser quirks (auto-allow dialog, tab close) automatically.

=== BEST PRACTICES ===
- Always use --json: agents parse JSON, not terminal text
- Always use --trace <file>: records tool usage for analysis
- Prefer search harvest over manual search+follow for multi-topic work (faster, fewer commands)
- Check follow quality.is_ok before processing content
- Use sections array when available for document structure awareness
- Start daemon once, reuse for all operations
- Set --max high enough (30000+) for long-form content
- For 3+ URLs, use follow batch instead of individual follow calls
```
