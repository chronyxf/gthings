# Page lifecycle: connect · navigate · tabs · iframes · wait

> Before reaching for these low-level CDP snippets, check if
> `browser-automation.ts` commands (`follow`, `search`, `batch-follow`,
> `batch-search`, `batch-harvest`) already cover your use case. Use these
> interaction skills only when the high-level commands don't support
> what you need — for example, custom click sequences, multi-step forms,
> or intercepting network requests.

## Connect

```js
await session.connect()                                    // auto-detect
await session.connect({ wsUrl: "ws://127.0.0.1:9222/..." })
await session.connect({ profileDir: "/path/to/Chrome" })
```

If auto-detect fails → open `chrome://inspect/#remote-debugging` (Chrome) or `dia://inspect/#remote-debugging` (Dia) → tick "Discover network targets" → Allow. The `session.connect()` error message (templates.ts:169) lists both URLs.

## Create tab + navigate

```js
const t = await session.Target.createTarget({ url: "about:blank", background: true })
await session.Target.attachToTarget({ targetId: t.targetId, flatten: true })
await session.use(t.targetId)
await session.Page.enable()
await session.Page.setLifecycleEventsEnabled({ enabled: true })
// Arm wait BEFORE navigate (race fix)
const ready = session.waitFor("Page.lifecycleEvent", p => p.name === "networkIdle", 30_000)
await session.Page.navigate({ url })
await ready
// Page is ready. Evaluate, screenshot, etc.
await session.Target.closeTarget({ targetId: t.targetId })  // cleanup
```

> **Note:** In modern browser-harness-js, use `session.createTarget(url)` (templates.ts:169) which combines `Target.createTarget` + `Target.attachToTarget` in one call and auto-registers the tab. The raw CDP calls above bypass the agent-tab registry, which means `session.closeTab()` with default `force:false` will reject with "Target is not an agent tab". Use `session.createTarget()` unless you have a specific reason to use the raw CDP calls.

Wait strategies: `"networkIdle"` (500ms quiet, default), `"networkAlmostIdle"` (250ms), `"load"`.

## Tab management

```js
const tabs = await listPageTargets()             // [{targetId,title,url,type}]
await session.use(tabs[0].targetId)              // switch to tab
const t2 = await session.Target.createTarget({ url: "https://..." })  // new tab
await session.use(t2.targetId)
await session.Target.closeTarget({ targetId: tid })   // close tab
```

`listPageTargets()` uses CDP `Target.getTargets`. Already filters `chrome://` and `devtools://`.

## Iframes (same-origin)

```js
// Get frames
const { frameTree } = await session.Page.getFrameTree()
// Find child frame
frameTree.childFrames[0].frame.id
// Navigate
await session.Page.navigate({ frameId: childFrameId, url })
// Evaluate in iframe
await session.Runtime.evaluate({ expression: "...", contextId })
```

## Iframes (cross-origin)

Cross-origin iframes block `Runtime.evaluate`. Use `Page.navigate` + events instead:

```js
// Enable events
await session.Page.enable()
// Listen for frame navigations
session.onEvent((m, p) => { if (m === "Page.frameNavigated") handle(p) })
```

## Dialog handling

```js
session.onEvent((m, p) => {
  if (m === "Page.javascriptDialogOpening") {
    session.Page.handleJavaScriptDialog({ accept: true, promptText: "" })
  }
})
```

Can't pre-subscribe before `Page.enable` — dialogs fire immediately.
