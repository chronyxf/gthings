# Troubleshooting

## Browser won't connect

**"Cannot connect on port 9222"** — no browser is running with CDP enabled. gsearch auto-launches on first use, but if the launch fails or a previous browser crashed, you may see this.

```bash
gsearch launch    # starts a fresh instance
```

If `gsearch launch` itself fails, check whether something is already on port 9222:

```bash
lsof -i :9222     # find what's using the port
kill <pid>        # kill the conflicting process
gsearch launch    # retry
```

**"Cannot connect on port 92229222"** — this is a sign of the old port-concatenation bug. The `_scan_ports` function had background processes leaking stdout (two `printf` outputs concatenated into one port string). Fix:

```bash
# Check common.sh line 232:
grep "probe_one" ~/.agents/skills/gsearch/lib/common.sh
# Should show: (_probe_one "$port" >/dev/null 2>&1) &
# If it shows: (_probe_one "$port") &
# → update via bootstrap.sh install
```

**"Session with given id not found"** — the CDP session was lost. The REPL server may have restarted or the browser tab was closed externally.

```bash
browser-harness-js --restart    # restart the REPL
gsearch launch                   # re-launch browser
```

## Browser pops up unexpectedly

The old gsearch code (before July 2026) opened `chrome://inspect/#remote-debugging` in the user's real browser, thinking the inspect toggle enables CDP. It doesn't — that toggle discovers REMOTE targets, it does NOT start a local CDP server. This created an infinite loop: pop up → user toggles → no CDP → next command pops up again.

The fix was to remove all `open -a` calls from `ensure_browser()` in `common.sh`. Now the tool never touches the user's real browser. If you still see popups:

1. Your gsearch is outdated. Run `./bootstrap.sh install` to update.
2. Something else on your system is opening the browser.

## Batch commands return empty or fail

**`batch search` returns `[]`** — all searches returned no results. Try the same query directly:
```bash
gsearch "same query"
```
If the direct search also returns `[]`, the query genuinely has no results — try different wording. If the direct search works but `batch search` doesn't, the extraction selector for Google results may have changed.

**`batch follow` returns empty content for a known URL** — several possible causes:

- **PDF URL** → Chrome viewer may not extract text. Try `gsearch pdftotext <url>`.
- **Paywalled page** → the content quality gate rejected it (< 80 chars). Try a different source.
- **JavaScript-rendered page** → some SPAs need extra time. For single pages, use `gsearch follow <url> --settle 5000` (`--settle` flag, see `follow.sh:35`).
- **Connection error** → the URL may be temporarily unreachable. Retry later.

**`batch harvest` shows `pages_skipped` nearly equal to `pages_read`** — too many URLs failed or returned low-quality content. Solutions:

- Use more specific queries that return higher-quality sources
- Increase `--count` to get more candidates, `--max` to read more
- Avoid queries that return mostly PDFs (like "research paper" or "study")
- Check the `_error` fields on individual `read_pages` entries to see why each one failed

## PDF extraction fails

**arXiv paper returns empty** — the URL rewrite may not have triggered. Check that the URL matches `arxiv.org/pdf/<id>`. If you have an `abs` URL already, `batch follow` will work directly:

```bash
gsearch follow "https://arxiv.org/abs/2401.12345"   # direct abstract page
```

**Non-arXiv PDF gets `_error: "pdf_textlayer_empty"`** — Chrome's PDF viewer couldn't extract the text layer. Try:

```bash
gsearch pdftotext "https://example.com/paper.pdf"
```

If `pdftotext` also returns empty, the PDF is likely scanned (image-only with no text layer) or encrypted. These can't be extracted with current tools.

**pdftotext itself fails** — check that curl is installed:
```bash
which curl   # should be on PATH
```

## Environment issues

**`browser-harness-js: not found`** — the CDP skill is not installed or not on PATH.

```bash
ls ~/.agents/skills/cdp/sdk/browser-harness-js   # check if it exists
./bootstrap.sh install                             # install skills
```

**Bun auto-install fails** — some corporate networks block the install script at `bun.sh`.

```bash
export BROWSER_HARNESS_SKIP_BUN_INSTALL=1   # skip auto-install
brew install oven-sh/bun/bun                 # install manually
```

**Chrome or Dia won't start** — the temp profile at `/tmp/gsearch-profile` may be locked from a previous crash.

```bash
rm -rf /tmp/gsearch-profile /tmp/gsearch-tokens   # clean up
gsearch launch                                      # fresh start
```

## Exit codes

| Code | Meaning | What you get |
|------|---------|--------------|
| 0 | Success | JSON on stdout |
| 1 | Usage error (bad arguments) | Error message on stderr |
| 2 | Browser or dependency error | JSON on stderr: `{"tool":"gsearch","error":"...","detail":"..."}` |
