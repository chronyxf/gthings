# gthings

Browser automation and web research toolkit. Single Rust binary, zero runtime dependencies.

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

# Search
./target/release/gthings --json search query "Rust async" --count 5

# Read a page
./target/release/gthings --json follow url "https://www.rust-lang.org" --max 20000

# Batch read
./target/release/gthings --json follow batch "url1" "url2" --max 20000

# Browse PDF
./target/release/gthings --json pdf url "https://arxiv.org/pdf/xxxx.xxxxx"

# Explicit browser lifecycle
./target/release/gthings --json browser status
./target/release/gthings browser stop
```

Add `--trace /tmp/trace.jsonl` to every command for step-level debugging.

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
| `search query <q> --count N` | Google search | `[{title, url, snippet}]` |
| `search batch <q1> <q2> --count N` | Multi-query search | `[{results[], meta}]` |
| `search harvest <q1> <q2> --count N --follow M` | Search + follow pipeline | `{search_results[], read_pages[], meta}` |
| `follow url <url> --max N` | Extract page content | `{url, content, sections[], quality, truncated}` |
| `follow batch <url1> <url2> --max N` | Multi-page extraction | `[{url, content, quality}...]` |
| `pdf url <url>` | Extract PDF text | `{content, pages, meta}` |
| `pdf file <path>` | Extract local PDF | `{content, pages, meta}` |
| `browser status` | Check browser state | `{status, pid, ws_url}` |
| `browser stop` | Kill browser | `{pid, status}` |

All commands support `--json` (structured output) and `--trace <file>` (step-level JSONL logging).
