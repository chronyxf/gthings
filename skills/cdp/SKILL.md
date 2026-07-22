---
name: cdp
description: "CDP browser automation via browser-harness-js CLI. 56 domains, 652 typed methods. Persistent session across calls."
compatibility: "Chromium with --remote-debugging-port. Bun required (auto-installed on first run)."
---

## Security

**CRITICAL:** Never connect CDP to Chrome running with your real profile.
CDP has full access to cookies, passwords, history, and can execute JS in any tab context.

Always use `gsearch launch` to start Chrome with an isolated temp profile at `/tmp/gsearch-profile`.

## The Core Rule

All browser automation goes through `browser-automation.ts`. It wraps every tab operation with timeouts, error isolation, and content quality gates. Never call `browser-harness-js` directly — it has none of these safeguards.

## When This Applies

| Use CDP when | Use gsearch instead |
|---|---|
| Single page follow, extract, screenshot | Multi-query web search with batch research |
| Raw CDP automation code | Batch follow with dedup and ranking |
| Tab lifecycle control (create, navigate, close) | arXiv auto-conversion, PDF extraction |
| Debugging browser interaction | Full research pipeline (search → dedup → extract) |
| — | Academic paper full-text: **gsearch pdftotext** (no 15k char cap vs batch-follow's 15k cap) |

## Core Commands

```
browser-automation.ts follow <url> [--selector S] [--offset N] [--max M] [--pretty] [--timeout MS] [--port N] [--raw]
browser-automation.ts batch-follow <url1> ... [--selector S] [--offset N] [--max M] [--pretty] [--timeout MS] [--port N]
browser-automation.ts search <query> [--count N] [--port N]
browser-automation.ts batch-search <q1> ... [--count N] [--port N]
browser-automation.ts batch-harvest <q1> ... [--count N] [--max M] [--timeout MS] [--port N]
```

### New flags:

| Flag | Default | Description |
|------|---------|-------------|
| `--offset N` | 0 | Character offset to start reading from. Allows incremental reading of long pages. |
| `--max M` | 15000 | Max characters to return. Set to -1 for unlimited (use with caution). |
| `--pretty` | off | Pretty-print JSON output for human readability. |

## Design Rationale

**Information theory.** Academic text entropy is ~1.4–1.7 bits/char. A complete idea unit (one coherent section) requires 3k–12k chars. The 15k cap captures 1–2 units — optimal signal-to-noise ratio for aggregation, before "Lost in the Middle" attention degradation (Liu et al. 2023) reduces retrieval accuracy beyond 4k tokens.

**Category theory.** Every extraction is a morphism f: URL → Content with provenance `{source, method, offset}`. `follow` and `batch-follow` are forgetful functors — bounded output (15k), fast, suitable for scanning. For full information preservation, compose with the gsearch skill's pdftotext command (faithful functor, no cap). The two functors form a commuting diagram: same source URL, different extraction depth, both traceable to origin.

## The Traps You Will Hit

### Trap 1: The Direct-Call Trap
You think calling `browser-harness-js` directly is faster. But it has no timeout wrapping, no error isolation, no quality gate. Every call must go through `browser-automation.ts`.
— `reference/multi-tab.md`, `SKILL.md` Core Rule

### Trap 2: The One-Shot Wait Trap
You think `session.waitFor()` registers a persistent listener. But it fires once on the currently active target. Switching tabs orphans the listener. Use polling (`document.readyState`) for multi-tab operations.
— `reference/multi-tab.md`

### Trap 3: The Leaking-Tab Trap
You think closing the script closes all tabs. They persist across calls. Every `createTarget()` needs a `closeTarget()` in try/finally.
— `reference/errors.md`

### Trap 4: The Content-Confidence Trap
You think extracted text is always usable. Many pages return error messages, paywalls, or empty shells. Always run the quality gate and check for `_error: "low_quality_content"`.
— `reference/content-quality.md`

### Trap 5: The PDF-Cap Trap
You think batch-follow on a PDF returns the full document. But content is capped at 15,000 characters. Academic papers routinely exceed this. You get a truncated paper with no error or warning. For full academic paper text, delegate to gsearch's pdftotext command — it has no character cap.
— `reference/content-quality.md`, `agents/skills/gsearch/SKILL.md` Trap 8

### Trap 6: The No-Incremental-Read Trap
You think setting `--max -1` gives you unlimited content. But large pages (100k+ chars) can slow CDP transfer and bloat context. Always use `--offset`/`--max` for incremental reading: start at 0/15000, check `truncated`, then request the next chunk. This keeps each chunk bounded and enables aggregators to build a section tree progressively.
— `extractionCode()`, `followCode()`

### Trap 7: The Dia-Space Trap
You think Dia Browser's Spaces (tab groups) isolate your agent to tabs within its own Space. They don't. Spaces exist at the browser UI layer only — CDP sees a flat list of ALL targets across ALL Spaces. Your agent can see, read, and close tabs in ANY Space. The agent-tab registry (session.ts) prevents closing tabs the agent didn't create, but there is no guard against *reading* content from user tabs in other Spaces. For true isolation, use isolated browser contexts (incognito profiles) — not Spaces.
— `session.ts`, CDP spec

## Playbook

1. **Launch browser** — `gsearch launch` (isolated Chrome on port 9222)
2. **Connect** — `browser-automation.ts` handles connection automatically
3. **Pick command** — follow (single URL), search (single query), batch-follow (multi URL), batch-search (multi query), batch-harvest (research pipeline)
4. **Extract** — content is quality-gated automatically
5. **Close** — tabs close in try/finally; REPL state persists across calls
6. **Verify** — check output for `_error` fields; retry with longer timeout if empty. For PDFs, check content length — if it ends abruptly near 15k chars, the full paper was truncated. Use gsearch pdftotext instead.
7. **Incremental reading** — When content.truncated is true, use `--offset N` to get the next chunk. Each call returns {content, total_length, truncated, sections} so you can track progress through the document tree.

## Decision Tree

| Task | Command |
|---|---|
| Single URL content | `browser-automation.ts follow <url>` |
| Single URL, incremental | `follow <url> --offset 15000 --max 5000` (next chunk) |
| Multiple URLs | `browser-automation.ts batch-follow <url1> <url2> ...` |
| Multiple URLs, incremental | `batch-follow <url1> ... --offset 15000 --max 5000` |
| Single Google search | `browser-automation.ts search <query> --count N` |
| Multiple Google searches | `browser-automation.ts batch-search <q1> <q2> ... --count N` |
| Full research pipeline | `browser-automation.ts batch-harvest <q1> ... --count N --max M` |
| PDF URL (preview/abstract) | Use follow or batch-follow; PDFs auto-detected. **Capped at 15,000 characters.** |
| PDF URL (full academic paper) | Use gsearch pdftotext via the gsearch skill — **no character cap**, full paper text |

## Reference Documents

| File | Content |
|---|---|
| `reference/multi-tab.md` | Multi-tab parallelism, polling vs waitFor, error isolation |
| `reference/errors.md` | CDP error codes, JS errors, REPL health, failure patterns |
| `reference/content-quality.md` | Content quality gate — detection patterns |
| `reference/workspace-management.md` | /tmp/nyx-search/ workspace, cleanup rules |
| `interaction-skills/01-page-lifecycle.md` | Tab creation, navigation, wait strategies, iframes, dialogs |
| `interaction-skills/02-page-content.md` | Text extraction, screenshots, PDF, scrolling, shadow DOM |
| `interaction-skills/03-user-interaction.md` | Click, type, select, drag-and-drop, file upload |
| `interaction-skills/04-network-data.md` | Cookies, network interception, downloads, credentials |

## State Persistence

REPL state persists across `browser-automation.ts` calls. The `session` object and `globalThis` variables accumulate. Use `browser-harness-js --restart` to drop all state.

## Testing

74 TypeScript tests at `tests/`: `bun test agents/skills/cdp/tests/`
Covers template function purity, session.ts hardening, repl.ts serialization.
