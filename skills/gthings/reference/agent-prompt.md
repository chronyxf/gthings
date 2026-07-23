# Agent Prompt Template for gthings

```
You have access to gthings, a web research toolkit. It launches Chrome on demand — no
daemon setup needed. ALWAYS use --json for structured output.

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
   Purpose: Read a page. Extracts main content via CSS selector.
   Check quality.is_ok — if false, content may be paywalled/captcha/too short.

5. gthings --json follow batch "url1" "url2" --max N
   Returns: [{...FollowResult...}, {...FollowResult...}]
   Purpose: Batch page reading. Use for 3+ independent URLs instead of individual follow calls.

6. gthings --json pdf url "<arxiv-url>"
   Returns: {"url":"...","text":"...","length":N,"source":"...","pages":N}
   Purpose: Extract text from PDFs (arXiv, research papers, etc.). /pdf/ to /abs/ rewrite automatic.

7. gthings --json pdf file "<path>"
   Returns: {"source":"/path/to/paper.pdf","text":"...","length":N,"pages":N}
   Purpose: Extract text from local PDF files. Pure Rust, no external deps.

8. gthings browser start | stop | status
   Purpose: Manage the persistent Chrome browser. Auto-started on first use.

=== WORKFLOW PATTERNS ===

For SINGLE-TOPIC research:
  1. search query "topic" --count 5
  2. Evaluate results by title/url/snippet
  3. follow url <each promising URL> --max 15000
  4. Repeat with refined queries if needed
  5. Synthesize findings

For MULTI-TOPIC research (3+ topics):
  1. Use search harvest "topic1" "topic2" "topic3" --count 5 --max 3
     Does Phase 1 (search all topics) + Phase 2 (follow top 3 per topic) in one command.
  2. For deeper follow on specific topics, use follow url individually
  3. Use follow batch for 3+ independent URLs

For PAPER/ACADEMIC research:
  1. search query "topic arxiv" --count 10
  2. pdf url <arxiv-link> for each paper found
  Note: pdf extraction is pure Rust — no browser needed.

=== ERROR HANDLING ===
- Chrome cold start: first call takes ~2s to launch headless Chrome. Subsequent calls reuse it.
- Empty search results: try with slightly different query wording
- quality.is_ok=false: content is low quality — try follow with --selector "body" --max 30000
- truncated=true: content was cut off at max length — increase --max
- pdf url errors: "HTTP 404" means wrong URL. "does not appear to be a PDF" means it's HTML.
  Use follow url instead for non-PDF URLs.

=== BEST PRACTICES ===
- Always use --json: agents parse JSON, not terminal text
- Always use --trace <file>: records tool usage for analysis
- Prefer search harvest over manual search+follow for multi-topic work (faster, fewer commands)
- Check follow quality.is_ok before processing content
- Use sections array when available for document structure awareness
- Chrome is auto-managed — use browser start/stop/status to control it
- Set --max high enough (30000+) for long-form content
- For 3+ URLs, use follow batch instead of individual follow calls
```
