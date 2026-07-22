# gthings

Browser automation and web research toolkit. Single Rust binary, zero runtime dependencies.

## Install

```bash
git clone https://github.com/chronyxf/gthings.git
cd gthings
cargo build --release
# Binary at target/release/gthings
# Daemon at target/release/browser-daemon
```

Requires Rust 1.85+. Chrome or any Chromium browser.

## Quick Start

```bash
# Start daemon
target/release/browser-daemon start --port 9222

# Search + read in one command
target/release/gthings --json search harvest "topic" --count 5 --max 3
```

## For AI Agents

After building, sync the skill to your global agent directory:

```bash
# Install skills globally (copies skills/gthings/ → ~/.agents/skills/gthings/)
bash scripts/install-skills.sh

# Or with a custom prefix
bash scripts/install-skills.sh --prefix ~/.config/opencode
```

Once installed, AI agents can load the skill from any project:

```
skill gthings
```

The skill includes:

| File | Use |
|------|-----|
| `SKILL.md` | Full playbook, traps, decision tree |
| `reference/commands.md` | Every command with JSON return types |
| `reference/agent-prompt.md` | Ready-to-use prompt for spawning agents |
| `reference/agent-trace.md` | --trace telemetry analysis |
| `reference/daemon.md` | Daemon lifecycle |
| `reference/quality.md` | Quality gate + section extraction |
| `reference/errors.md` | Troubleshooting |

## Troubleshooting

**Rust version too old** — This project requires Rust 1.85+ (edition 2024).

```bash
rustc --version        # Check version
rustup update stable   # Update to latest
rustup default stable  # Set stable as default
```

**Build fails** — Check your toolchain:

```bash
rustup toolchain list                   # Verify installed toolchains
cargo check 2>&1 | head -20             # See specific errors
rustup component add clippy rustfmt     # Install required components
```

**Daemon won't start** — Port conflict or browser not found:

```bash
lsof -i :9222                           # Check port usage
which chrome || which chromium || true  # Check browser path
gthings browser start --port 9223       # Try alternate port
```

## Commands

All support `--json` (structured output) and `--trace <file>` (telemetry).

```
search query    Single Google search
search batch    Multi-query search
search harvest  Two-phase search + follow
follow url      Read a page (sections + quality gate)
follow batch    Read multiple pages
pdf url         Extract text from PDF URL
pdf file        Extract text from local PDF
screenshot      Capture PNG (or --json base64)
scrape          Extract elements by CSS selector
browser         Daemon management (start/stop/status/logs/call/eval)
```
