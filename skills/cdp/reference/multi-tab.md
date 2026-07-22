> NOTE: `scripts/browser-automation.ts` handles multi-tab operations automatically. This reference is for understanding the underlying mechanism.

# Multi-tab parallelism

browser-harness-js is a single-threaded Bun REPL server. Every JS snippet you send via `browser-harness-js '...'` is an HTTP POST to `/eval` — they queue and run serially. Shell `&` does not help.

True parallelism means creating multiple CDP targets (tabs) within **one** JS invocation, then navigating them all before waiting for any of them.

## The pattern: create → navigate → extract

```
// Phase 1: create all tabs (fast — no network)
for each url:
  session.Target.createTarget({url:"about:blank",background:true})
  session.Target.attachToTarget({targetId, flatten: true})

// Phase 2: navigate ALL tabs (fast — returns immediately, pages load in background)
for each tab:
  session.use(tab.targetId)
  session.Page.enable()
  session.Page.navigate({url: tab.url})

// Phase 3: extract all (pages loaded concurrently — polls find ready state fast)
for each tab:
  session.use(tab.targetId)
  poll for document.readyState === "complete"
  session.Runtime.evaluate({expression: "..."})
```

The total time is approximately `max(page_load_times) + N * 0.1s` — not `sum(page_load_times)`.

## Why polling, not waitFor

`session.waitFor()` registers a one-shot event listener on the **currently active target**. Consider this scenario:

```javascript
// Tab A navigates
session.use(tabA);
const readyA = session.waitFor("Page.lifecycleEvent", p => p.name === "networkIdle");
await session.Page.navigate({url: urlA});

// Switch to Tab B
session.use(tabB);
const readyB = session.waitFor("Page.lifecycleEvent", p => p.name === "networkIdle");
await session.Page.navigate({url: urlB});

await readyA;  // TIMEOUT — waitFor was registered on tabB, not tabA!
```

When you call `session.use(tabB)`, the active target changes. The `waitFor` on the next line registers for tabB, not tabA. TabA's `networkIdle` event already fired or will fire on tabA's session, but the listener is on tabB's session. You miss it.

**Solution:** Poll for `document.readyState` instead. It works regardless of which target is active:

```javascript
for (let i = 0; i < 50; i++) {
  try {
    const r = await session.Runtime.evaluate({expression: "document.readyState", returnByValue: true});
    if (r.result?.value === "complete") break;
    if (r.result?.value === "interactive" && i > 15) break; // good enough after ~3s
  } catch(e) {
    break; // tab crashed — stop polling
  }
  await new Promise(r => setTimeout(r, 200));
}
```

The catch: `waitFor` catches events that fired while polling, so we combine them:

```javascript
// Best approach: try waitFor first (fast path), fall back to polling
try {
  await session.waitFor("Page.lifecycleEvent", p => p.name === "networkIdle", 5000);
} catch(e) {
  // Timeout — event already fired or hasn't fired yet. Poll to be sure.
  for (let i = 0; i < 30; i++) {
    const r = await session.Runtime.evaluate({expression: "document.readyState === 'complete'", returnByValue: true});
    if (r.result?.value) break;
    await new Promise(r => setTimeout(r, 200));
  }
}
```

## Per-tab error isolation

Every tab in a batch must be wrapped in try/catch. A single `ERR_CONNECTION`, DNS failure, or timeout must never crash the entire batch.

```javascript
const results = [];
for (const tab of tabs) {
  let content = "";
  let error = null;
  try {
    await session.use(tab.targetId);
    for (let i = 0; i < 50; i++) {
      const r = await session.Runtime.evaluate({expression: "document.readyState", returnByValue: true});
      if (r.result?.value === "complete") break;
      await new Promise(r => setTimeout(r, 200));
    }
    const r = await session.Runtime.evaluate({expression: "document.body.innerText", returnByValue: true});
    content = (r.result?.value || "").slice(0, 15000);
  } catch(e) {
    error = e.message;
  }
  results.push({
    url: tab.url,
    content: error ? "" : content,
    ...(error ? {_error: error} : {})
  });
}
```

If you don't isolate per tab, one bad URL kills the entire batch and you get zero results.

## Tab lifecycle

Tabs persist across `browser-harness-js` calls. If you create a tab in one call and don't close it, it's still there in the next call. Use `listPageTargets()` to see what's open:

```javascript
const tabs = await listPageTargets();
// → [{targetId, title, url, type}]
```

Close tabs when done to avoid leaking:

```javascript
try { await session.Target.closeTarget({targetId: tab.targetId}); } catch(e) {}
```

The `try/catch` around close is important — the tab may have already crashed or been closed externally.
