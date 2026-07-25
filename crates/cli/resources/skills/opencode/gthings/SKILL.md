---
name: gthings
description: "Browser automation and web research CLI — search, follow, PDF extraction, batch harvest, quality gate, telemetry"
---

# Skill: gthings

Browser automation and web research toolkit. Native Rust binary — single static binary that launches Chrome on demand.

## Core Rules

1. **Always use `--json`** — every command outputs structured JSON.
2. **Prefer `search harvest`** for multi-topic research — one command replaces search×N + follow×M.
3. **Use `--trace <file>`** to record all commands for observability (appends JSONL).
4. **Batch independent follows** — use `follow batch` for 3+ URLs.
5. **Expect ~2s cold start** — first call launches Chrome. Subsequent calls reuse it.
6. **Check `quality.is_ok`** before processing followed content.

## Installation

```bash
cargo install gthings
```

After installation, run `gthings update` to configure your shell and install skills:

- Detects your shell (bash, zsh, fish)
- Adds `~/.cargo/bin` to your PATH (auto-edits shell config file)
- Installs gthings skills to opencode and agents directories

```bash
gthings update
```

Then restart your terminal or source your config file.

## Commands (all support `--json` and `--trace <file>`)

**update** — one-command update: upgrades binary, configures shell PATH, installs skills.
**skill add** — install gthings skills to `~/.config/opencode/skills/` or `~/.agents/skills/`.
**search query** `<query>` `--count N` — single Google search.
```json
{"meta":{"total":5,"query":"...","duration_ms":2340},"results":[{"title":"...","url":"...","snippet":"..."}]}
```
**search batch** `"q1" "q2"` `--count N` — multi-query, dedup by URL. `{"results":[...],"meta":{...}}`
**search harvest** `"q1" "q2"` `--count N --max M` — two-phase: search + follow top M/query. `{"search_results":[...],"read_pages":[...],"meta":{...}}`
**follow url** `<url>` `--max N --selector S --offset O` — read a page. `{"success":true,"url":"...","content":"...","sections":[...],"quality":{...}}`
**follow batch** `"url1" "url2"` `--max N` — batch reading. `[{...},{...}]`
**pdf url** `<url>` — PDF text extraction. arXiv auto-rewritten. `{"source":"...","text":"...","length":N,"pages":N}`
**pdf file** `<path>` — local PDF. Pure Rust, offline. `{"source":"...","text":"...","length":N,"pages":N}`
**browser start|stop|status** — manage persistent Chrome. `{"status":"started|stopped|running","pid":N,"ws_url":"..."}`

## Content Quality Gate

Every `follow` result includes `quality: {score, is_ok, reasons, length}`. Check `quality.is_ok` before processing. See `reference/quality.md` for thresholds.

## Telemetry

`--trace <file>` appends one JSONL line per command:
```json
{"ts":"1784705060.276","session":"ses_...","tool":"search","args":{...},"duration_ms":2340,"exit":0}
```
Fields: ts (unix.nanos), session (per-process hex ID), tool, args, duration_ms, exit (0=ok,1=error).

## Error Handling

| Error | One-line fix |
|-------|-------------|
| PDF URL 404 | Use abstract page URL for arXiv; verify URL |
| HTML not PDF | Use `follow url` instead of `pdf url` |
| Failed to extract PDF text | Use HTML/abstract version |
| Empty search results | Retry with different wording |
| Sign-in in results | Titles/URLs still usable |
| `truncated: true` | Increase `--max` or use `--offset` |
| `sections: []` | `content` field still has full text |
| `quality.is_ok: false` | Retry with `--selector "body" --max 30000` |
| Browser not working | `browser status; browser stop; browser start` |
| PDF extraction empty | Scanned PDF — try abstract version |

## Traps

1. **Chrome cold start**: First call ~2s. Use `browser status` before working.
2. **Empty search results**: Rate-limiting or network. Retry with different wording.
3. **Low quality content** (`quality.is_ok=false`): Retry `--selector "body" --max 30000`.
4. **Truncated content** (`truncated: true`): Increase `--max` or use `--offset`.
5. **Empty sections** (`sections: []`): No h1/h2/h3. Content in `content` field.
6. **PDF URL not PDF**: Use `follow url` for HTML pages.
7. **Browser not stopping**: `browser stop` kills process. If stuck, `kill <pid>`.

## Environment Variables

| Variable | Default | Purpose |
|----------|---------|---------|
| `GTHINGS_CACHE_DIR` | `/tmp/gthings-cache` | Disk cache directory |
| `GTHINGS_CACHE_TTL_SECS` | `3600` | Cache TTL |
| `GTHINGS_CDP_PORT` | `9222` | Chrome remote debugging port |
| `GTHINGS_LOG_LEVEL` | `info` | Log level |
| `GTHINGS_PER_HOST_RATE` | `2` | Requests/sec per host |
| `GTHINGS_PER_HOST_BURST` | `5` | Max burst per host |
