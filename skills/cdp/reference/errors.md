# Error handling

## CDP error codes

| Code | Meaning | Most likely cause | Fix |
|------|---------|-------------------|-----|
| `-32000` | Method not found | Typo in domain or method name `Page.navigat` vs `Page.navigate` | Check the exact method name in `generated.ts` |
| `-32001` | Session with given id not found | Tab was closed, or `session.use()` was called with a stale targetId | Re-create the tab, or restart the REPL with `browser-harness-js --restart` |
| `-32002` | Target closed | Tab crashed or was closed by another process | Create a new target |
| `-32100` | Invalid params | Missing required field or wrong type passed to a CDP method | Check the method signature |
| Timeout | Event not received | `session.waitFor()` was called but the event never fired, or it fired on a different target | Use polling instead of waitFor for multi-tab workflows |

## JavaScript errors from the REPL

| Error you see | What it means | Fix |
|---------------|--------------|-----|
| `Cannot connect on port N` | `session.connect({port: N})` failed — no browser responding on that port | Run `gsearch launch` to start a browser, or check the port number |
| `session is not defined` | The REPL was restarted and the session global was lost | `browser-harness-js --restart` |
| `SyntaxError: Unexpected token` | The JS code sent to the REPL has a syntax error | Check your string escaping. If you're embedding JS in a bash heredoc, use `<<'EOF'` (single-quoted delimiter prevents shell expansion) |
| `Cannot read properties of undefined (reading 'result')` | `r.result` was undefined — the evaluation failed | Check `r.exceptionDetails` for the actual error from the page |
| CDP -32001 (in multi-tab workflows) | A tab crashed, and you're trying to `session.use()` it | Wrap all per-tab operations in try/catch |

## REPL health codes

`browser-harness-js --status` returns a JSON health check:

```json
{"ok":true,"uptime":N}    // REPL is running, N seconds uptime
{"ok":false,"error":"down"} // REPL is not running
```

If the REPL is down:
```bash
browser-harness-js --start     # start it
browser-harness-js --restart   # stop + start fresh (drops all state)
```

The REPL log is at `/tmp/browser-harness-js.log` — check this for detailed errors.

## Common failure patterns

**waitFor timeout on the second tab:**
You navigated tab A, called waitFor, then switched to tab B. The waitFor was registered on tab A, but after `session.use(tabB)`, the listener was switched to tab B. Tab A's event was never caught. Fix: use polling instead of waitFor (see `reference/multi-tab.md`).

**session.use() with a crashed tab:**
A tab that received `ERR_CONNECTION` or was closed externally will throw CDP -32001 when you call `session.use()`. Always wrap per-tab operations in try/catch.

**Unclosed tabs accumulating:**
Each `session.Target.createTarget()` creates a new tab in the browser. If you don't close them, they accumulate and consume memory. Always close tabs when done:
```javascript
try { await session.Target.closeTarget({targetId}); } catch(e) {}
```
