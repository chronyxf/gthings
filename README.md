# gthings

Single Rust binary, zero external dependencies.

## Architecture

```
AI Agent → gthings CLI
               │
               ├── Persistent Dia/Chrome (port 9222)
               ├── Tab create → navigate → extract → tab close
               └── --json output
```

## Install

### From crates.io

```bash
cargo install gthings
```

### From source

```bash
git clone https://github.com/chronyxf/gthings
cd gthings
cargo build --release
./target/release/gthings --help
```

Requires Rust 1.85+ and a Chromium-based browser (Dia, Chrome, Brave, Edge).

## Quick Start

```bash
# Browser auto-launches on first command, persists across calls

# Simple search (5 results by default)
./target/release/gthings search "Rust async" --json

# Multi-query search (parallel)
./target/release/gthings search "Rust async" "Tokio tutorial" --strategy parallel --json

# Search + follow top results (harvest)
./target/release/gthings search "rust borrow checker" --strategy harvest --follow-top 5 --json

# Extract content from a URL
./target/release/gthings extract "https://www.rust-lang.org" --max-chars 20000 --json

# Accessibility tree
./target/release/gthings ax "https://www.rust-lang.org" --json

# Extract PDF from URL
./target/release/gthings pdf-url "https://arxiv.org/pdf/2401.12345" --json

# Extract PDF from local file
./target/release/gthings pdf-file "paper.pdf" --json

# Check browser status
./target/release/gthings status --json

# Update gthings
./target/release/gthings update
```

All commands accept `--output text|json|nd-json` (or legacy `--json`) and `--query <dot-notation>` for field filtering (a custom dot-notation subset, e.g. `.data`, `.[].url`, `.results[].snippet` — not full JMESPath).

## For AI Agents

Install the gthings skill so AI agents know how to use the tool:

```bash
bash scripts/install-skills.sh
```

Then agents load it via `skill gthings`. The skill provides:
- Command reference with flags and JSON return types
- Quality gate documentation (how content is scored)
- Agent prompt template with workflow patterns
- Error code reference for troubleshooting
- Trace telemetry format for analyzing agent behavior

## Commands

| Command | Description | JSON Output |
|---------|-------------|-------------|
| `search <q> --strategy simple` | Google search | `{"results": [...], "query": "..."}` |
| `search <q1> <q2> --strategy parallel` | Multi-query parallel search | `{"results": [...]}` |
| `search <q1>... --strategy harvest --follow-top M` | Search + follow pipeline | `{"results": [...], "summary": {...}}` |
| `extract <url>` | HTTP/Readability extraction (auto-detects PDF, arXiv, GitHub) | `{title, body, quality, provenance}` |
| `ax <url>` | Accessibility tree | `{tree, url, total_nodes, truncated}` |
| `pdf-url <url>` | PDF from URL | `{Pdf:{pages, text}, quality}` |
| `pdf-file <path>` | Local PDF file | `{Pdf:{pages, text}, quality}` |
| `status` | Browser connection check | `{browser, status, version}` |
| `update` | Update gthings to latest version | version info |

All commands support `--output text|json|nd-json` (or `--json` for JSON output) and `--query <dot-notation>` for field filtering (a custom dot-notation subset, not JMESPath).

## Output Envelope

Every command emits a `{status, data, error}` envelope so agents have one parse path regardless of success or failure:

```json
{
  "status": "ok" | "error",
  "data": <command-specific result>,
  "error": {
    "code": "ERROR_CODE",
    "detail": "human-readable detail",
    "hint": "recovery hint"
  }
}
```

On success `status` is `"ok"` and `error` is `null`; on failure `status` is `"error"` and `data` is `null`. Use `--query .data` to unwrap the payload.

## Search Operators

Queries support standard search operators (rewritten per-engine; unsupported operators are stripped rather than failing):

| Operator | Example | Meaning |
|----------|---------|---------|
| `site:` | `rust site:github.com` | Restrict to a domain |
| `-exclusion` | `rust -tutorial` | Exclude a term |
| `"quoted"` | `"borrow checker"` | Exact phrase |
| `filetype:` | `rust filetype:pdf` | Restrict to a file type |
| `intitle:` | `intitle:async rust` | Term in page title |
| `inurl:` | `inurl:docs rust` | Term in URL |
| `AROUND(n)` | `docker AROUND(3) compose` | Terms within n words |
| `before:` / `after:` | `rust after:2024` | Date range filter |
| `OR` / `AND`, `(...)` | `(docker OR podman) compose` | Boolean grouping |

`--engine auto|brave|bing|google` selects the search engine. Brave and Bing are plain-HTTP (no browser needed); Google requires a CDP browser; `auto` degrades to HTTP engines when no browser is available.
