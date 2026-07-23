# Agent Telemetry: --trace Flag

## Purpose

The `--trace <file>` flag records every gthings command as a JSONL line. This enables:

1. **Observability**: See exactly what commands agents run, in what order
2. **Debugging**: Identify which URLs/tools fail and why
3. **Optimization**: Find repeated patterns that could be batched
4. **Behavior analysis**: Understand how agents use tools for different research tasks

## Format

One JSON object per line, newline-delimited (JSONL):

```json
{"ts":"1784705060.276456000","session":"ses_18c48bcbea5476e8","tool":"search","args":{"count":5,"query":"fed rate 2026"},"duration_ms":2340,"exit":0}
```

## Fields

| Field | Type | Description |
|-------|------|-------------|
| `ts` | string | Unix timestamp with nanosecond precision (`.276456000`) |
| `session` | string | Per-process identifier (nanotimestamp hex). One per CLI invocation. |
| `tool` | string | Command name |
| `args` | object | Key arguments (query, url, count, max, etc.) |
| `duration_ms` | integer | Wall-clock execution time in milliseconds |
| `exit` | integer | 0 = success, 1 = error |

## Usage

### Record all commands during research

```bash
gthings --trace /tmp/research.jsonl --json search harvest "topic1" "topic2" --count 5 --max 3
gthings --trace /tmp/research.jsonl --json follow url "https://..." --max 20000
```

Multiple invocations append to the same file.

### Analyze trace data

```bash
cat /tmp/research.jsonl | python3 -c "
import json, sys, collections

records = [json.loads(l) for l in sys.stdin]
tools = collections.Counter(r['tool'] for r in records)
total_ms = sum(r['duration_ms'] for r in records)
errors = sum(1 for r in records if r['exit'] != 0)

print(f'Total commands: {len(records)}')
print(f'Total time: {total_ms/1000:.1f}s')
print(f'Errors: {errors}')
print(f'Tools: {dict(tools)}')
print(f'Avg duration: {total_ms/len(records):.0f}ms')
"
```

### Identify slow operations

```bash
cat /tmp/research.jsonl | python3 -c "
import json, sys
records = [json.loads(l) for l in sys.stdin]
records.sort(key=lambda r: r['duration_ms'], reverse=True)
print('Top 5 slowest commands:')
for r in records[:5]:
    print(f\"  {r['tool']:20s} {r['duration_ms']:6}ms  {str(r['args'])[:80]}\")
"
```

### Reconstruct agent workflow

```bash
cat /tmp/research.jsonl | python3 -c "
import json, sys
records = [json.loads(l) for l in sys.stdin]
records.sort(key=lambda r: r['ts'])
print('Agent workflow:')
for r in records:
    print(f\"  {r['tool']:20s} exit={r['exit']} {r['duration_ms']:6}ms  {str(r['args'])[:60]}\")
"
```

## Performance Baseline (from 50-command finance research trace)

| Metric | Value |
|--------|-------|
| Total commands | 50 |
| Success rate | 96% |
| Avg duration | 2,938ms |
| Total agent time | 146.9s |
| Wall clock | 231s |
| Pages read | 31 |

Tool distribution:

| Tool | Count | % | Avg duration |
|------|-------|---|-------------|
| `follow` | 31 | 62% | 3,164ms |
| `search` | 10 | 20% | 1,393ms |
| `search_harvest` | 3 | 6% | 10,519ms |
| `pdf_url` | 4 | 8% | 484ms |
| `scrape` | 1 | 2% | 1,387ms |
| `browser_status` | 1 | 2% | 0ms |
