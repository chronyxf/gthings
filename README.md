# gthings

Single Rust binary, zero external dependencies.

## Architecture

```
AI Agent → gthings CLI
               │
               ├── Persistent Dia/Chrome (port 9222)
               ├── Tab create → navigate → extract → tab close
               └── --json output, --trace logging
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

Add `--trace /tmp/trace.jsonl` to every command for step-level debugging.

All commands accept `--output text|json|nd-json` (or legacy `--json`) and `--query <JMESPath>` for field filtering.

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
| `search <q> --strategy simple` | Google search | `[{title, url, snippet, position}]` |
| `search <q1> <q2> --strategy parallel` | Multi-query parallel search | `{queries: [{query, results}]}` |
| `search <q1>... --strategy harvest --follow-top M` | Search + follow pipeline | `{results[], summary}` |
| `extract <url>` | HTTP/Readability extraction (auto-detects PDF, arXiv, GitHub) | `{title, body, quality, provenance}` |
| `ax <url>` | Accessibility tree | `{tree, url, total_nodes, truncated}` |
| `pdf-url <url>` | PDF from URL | `{Pdf:{pages, text}, quality}` |
| `pdf-file <path>` | Local PDF file | `{Pdf:{pages, text}, quality}` |
| `status` | Browser connection check | `{browser, status, version}` |
| `update` | Update gthings to latest version | version info |

All commands support `--output text|json|nd-json` (or `--json` for JSON output), `--query <JMESPath>` for field filtering, and `--trace <file>` for step-level JSONL logging.
