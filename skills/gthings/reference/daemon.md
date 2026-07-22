# Daemon Lifecycle and Protocol

## Architecture

```
gthings CLI ──NDJSON over UDS──→ browser-daemon (persistent)
                                      │
                                      ├── cdp-core (WebSocket)
                                      │       └── Chrome/Dia CDP (port 9222)
                                      │
                                      ├── Tab lifecycle: createTarget("about:blank")
                                      │   → attachToTarget(flatten:true) → Page.enable
                                      │   → navigate → poll readyState → extract → closeTab
                                      │
                                      └── Tab close: window.close() + 100ms + Target.closeTarget
                                           (Dia Browser quirk — requires two-phase close)
```

## Starting the Daemon

```bash
gthings browser start --port 9222
```

The daemon:
1. Scans for running Chrome/Dia on port 9222 (3-endpoint probe: `/json/version` → `/json` → `/json/list`)
2. If not found, launches a new Chrome instance with `--remote-debugging-port=9222`
3. Connects via WebSocket to the browser's CDP
4. Creates the UDS socket at `/tmp/gthings-daemon.sock`
5. Writes PID to `/tmp/gthings-daemon.pid`

## UDS Protocol

The CLI sends JSON-RPC-style NDJSON (one JSON object per line, newline-delimited):

```json
{"id":1,"method":"follow","params":{"url":"https://example.com","max_length":15000}}
```

Response:

```json
{"id":1,"ok":true,"result":{"success":true,"url":"...","content":"...",...}}
```

Error:

```json
{"id":1,"ok":false,"error":"daemon not connected"}
```

### Supported Methods

| Method | Params | Description |
|--------|--------|-------------|
| `search` | `{query, count}` | Single Google search |
| `follow` | `{url, selector, offset, max_length}` | Page content extraction |
| `screenshot` | `{url}` | Page screenshot (returns base64 PNG) |
| `scrape` | `{url, selector, attribute}` | CSS selector extraction |
| `status` | `{}` | Daemon status |
| `stop` | `{}` | Graceful shutdown |

## Tab Lifecycle (per operation)

Each search/follow/screenshot/scrape creates a fresh tab and closes it:

```
1. Target.createTarget({"url": "about:blank"})    ← blank tab first (Dia optimization)
2. Target.attachToTarget({"targetId": id, "flatten": true})
3. Page.enable()                                   ← enable BEFORE navigation
4. Page.navigate({"url": target_url})
5. Poll document.readyState every 500ms (max 30-60 iterations)
6. Runtime.evaluate()                              ← extract content
7. Runtime.evaluate("window.close()")              ← Dia quirk: JS close first
8. sleep(100ms)                                    ← let browser process
9. Target.closeTarget({"targetId": id})            ← CDP teardown
```

## Port Management

The daemon detects Chrome/Dia on three possible debugging endpoints:
- `http://127.0.0.1:9222/json/version` (Dia returns 404 here)
- `http://127.0.0.1:9222/json` (lists targets)
- `http://127.0.0.1:9222/json/list` (alternative target list)

Dia Browser auto-allow dialog is dismissed via osascript on macOS during WebSocket connection.

## Logging

Daemon logs go to stdout (visible via `gthings browser logs`). Component-level logging:
- `info`: Operation start/end, connection state changes
- `debug`: CDP call details, timing, cache hits
- `warn`: Quality gate failures, retries
- `error`: Connection failures, unexpected CDP errors

## Multiple CLI Calls

The daemon handles one request at a time (sequential). Each CLI invocation:
1. Opens a UDS connection
2. Sends one NDJSON request
3. Reads one NDJSON response
4. Closes the connection

The daemon maintains the browser connection across CLI calls. Tab lifecycle is per-request.

## Cleanup

```bash
gthings browser stop    # graceful shutdown (waits for pending operations)
kill <pid>              # force kill if needed
rm /tmp/gthings-daemon.sock  # clean up stale socket
```

The daemon handles SIGTERM gracefully (closes CDP connection, removes socket).
