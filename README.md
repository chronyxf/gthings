# gthings

A browser-automated web research toolkit. One binary, two modes: **CLI** and **Docker daemon**.

## What it is

gthings turns a browser (Chrome/CDP) and plain-HTTP engines into a single research tool for AI agents and scripts. Every command returns one JSON envelope, so there is a single parse path regardless of success or failure.

## Install

```bash
cargo install gthings
```

Requires Rust 1.85+ and a Chromium-based browser (Chrome, Brave, Edge) for browser-backed commands.

## Quick start

```bash
gthings search "rust programming" --output json
```

Chrome/CDP is used when needed (google engine, `ax`); HTTP engines (bing, brave) work without a browser.

## Core capabilities

- **search** — strategies `simple` / `parallel` / `harvest`; engines `auto` → `google` / `bing` / `brave`, plus `brave-api` / `tavily` for paid keys.
- **extract** — web article / PDF / arXiv / GitHub content extraction.
- **ax** — accessibility tree.
- **pdf-url** / **pdf-file** — PDF extraction from a URL or local file.
- **batch / harvest** — follow-up content extraction on search results.

## Search strategies + engines

| Strategy | What it does |
|----------|--------------|
| `simple` | Single-query search |
| `parallel` | Multi-query search |
| `harvest` | Search + follow top results (`--follow-top M`) |

| Engine | Notes |
|--------|-------|
| `auto` | Default; degrades to HTTP engines when no browser is available |
| `google` | Requires a CDP browser |
| `bing` / `brave` | Plain HTTP, no browser needed |
| `brave-api` / `tavily` | Paid API keys |

Select with `--engine <engine>` and `--strategy <strategy>`.

## Output

Every command emits a single JSON envelope:

```json
{ "status": "ok" | "error", "data": <result>, "error": { "code", "detail", "hint" }, "trace_id": "..." }
```

Use `--output text|json|ndjson` to choose the format.

## Daemon / Docker

```bash
docker run -p 9080:9080 -e GTHINGS_ENGINE_MODE=free datnguyennnx/gthings:latest
```

Environment variables:

- `GTHINGS_ENGINE_MODE` — `free` / `hybrid` / `api`
- `GTHINGS_CDP_HOST` / `GTHINGS_CDP_PORT` — CDP browser connection
- `GTHINGS_SERVE_BIND` — daemon bind address

Endpoints: `/healthz`, `/metrics`, `POST /job`.

## Ecosystem

Six crates on crates.io: `gthings-common`, `gthings-extraction`, `gthings-cdp`, `gthings-search`, `gthings-serve`, `gthings`.

- Docs: `gthings describe` exposes full command reference and flags.
- Versioning / release workflow: [VERSION.md](VERSION.md).