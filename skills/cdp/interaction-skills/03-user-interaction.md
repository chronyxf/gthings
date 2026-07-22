# User interaction: click · type · select · dropdowns · DnD · dialogs

> Before reaching for these low-level CDP snippets, check if
> `browser-automation.ts` commands (`follow`, `search`, `batch-follow`,
> `batch-search`, `batch-harvest`) already cover your use case. Use these
> interaction skills only when the high-level commands don't support
> what you need — for example, custom click sequences, multi-step forms,
> or intercepting network requests.

## Click element

```js
// Simple click via JS (works for most cases)
await session.Runtime.evaluate({
  expression: "document.querySelector('selector').click()"
})

// CDP click (coordinates + events)
const rect = (await session.Runtime.evaluate({
  expression: "JSON.stringify(document.querySelector('selector').getBoundingClientRect())",
  returnByValue: true
})).result.value
const { x, y, width, height } = JSON.parse(rect)
await session.Input.dispatchMouseEvent({
  type: "mousePressed", x: x + width/2, y: y + height/2, button: "left", clickCount: 1
})
await session.Input.dispatchMouseEvent({
  type: "mouseReleased", x: x + width/2, y: y + height/2, button: "left", clickCount: 1
})
```

## Type into input

```js
// Clear then type
const sel = "input[name='q']"
await session.Runtime.evaluate({
  expression: `var el=document.querySelector('${sel}');el.value='';el.focus()`
})
await session.Input.dispatchKeyEvent({ type: "keyDown", text: "hello world" })
await session.Input.insertText({ text: "hello world" })
```

## Select dropdown option

```html
<select id="country"><option value="vn">Vietnam</option></select>
```

```js
// Native select
await session.Runtime.evaluate({
  expression: "document.querySelector('#country').value = 'vn'"
})

// If React/custom dropdown needs click-chain
// 1. click to open, 2. wait for options, 3. click option
await session.Runtime.evaluate({
  expression: "document.querySelector('.dropdown-toggle').click()"
})
// wait...
await session.Runtime.evaluate({
  expression: "[...document.querySelectorAll('.dropdown-item')].find(el => el.textContent.includes('Vietnam')).click()"
})
```

## Drag and drop

```js
// Get source + target positions
const src = JSON.parse((await session.Runtime.evaluate({
  expression: "JSON.stringify(document.querySelector('#drag-el').getBoundingClientRect())",
  returnByValue: true
})).result.value)
const dst = JSON.parse((await session.Runtime.evaluate({
  expression: "JSON.stringify(document.querySelector('#drop-zone').getBoundingClientRect())",
  returnByValue: true
})).result.value)

// Dispatch mouse events for drag
const steps = 20
for (let i = 0; i <= steps; i++) {
  const x = src.x + src.width/2 + (dst.x - src.x) * i / steps
  const y = src.y + src.height/2 + (dst.y - src.y) * i / steps
  await session.Input.dispatchMouseEvent({
    type: i === 0 ? "mousePressed" : "mouseMoved",
    x: Math.round(x), y: Math.round(y), button: "left"
  })
}
await session.Input.dispatchMouseEvent({
  type: "mouseReleased", x: Math.round(dst.x + dst.width/2),
  y: Math.round(dst.y + dst.height/2), button: "left"
})
```

## Dialog (alert/confirm/prompt) handling

```js
// Subscribe BEFORE the action that triggers the dialog
session.onEvent((method, params) => {
  if (method === "Page.javascriptDialogOpening") {
    session.Page.handleJavaScriptDialog({ accept: true, promptText: "" })
  }
})
await session.Page.enable()
// Now trigger the dialog...
```

If you subscribe after `Page.enable`, dialogs that fire synchronously on navigation may be missed.

## Upload file

```js
// Set file on input[type=file]
const { nodeId } = await session.DOM.querySelector({ nodeId: rootId, selector: "input[type=file]" })
await session.DOM.setFileInputFiles({ nodeId, files: ["/path/to/file.pdf"] })
```
