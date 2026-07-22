# Page content: text · screenshots · PDF · scrolling · shadow DOM · viewport

> Before reaching for these low-level CDP snippets, check if
> `browser-automation.ts` commands (`follow`, `search`, `batch-follow`,
> `batch-search`, `batch-harvest`) already cover your use case. Use these
> interaction skills only when the high-level commands don't support
> what you need — for example, custom click sequences, multi-step forms,
> or intercepting network requests.

## Extract text

```js
// Full page text
(await session.Runtime.evaluate({
  expression: "document.body.innerText",
  returnByValue: true
})).result.value

// CSS selector text
(await session.Runtime.evaluate({
  expression: "document.querySelector('article,main')?.innerText || document.body.innerText",
  returnByValue: true
})).result.value

// Multiple elements (JSON array)
(await session.Runtime.evaluate({
  expression: "JSON.stringify([...document.querySelectorAll('h2')].map(h => h.textContent.trim()))",
  returnByValue: true
})).result.value
```

## Screenshot

```js
const { data } = await session.Page.captureScreenshot({ format: "png" })
// data is base64-encoded. Decode: echo '<data>' | base64 -d > out.png
// Options: format ("png"|"jpeg"), quality (0-100, jpeg only),
// clip ({x,y,width,height,scale}), fromSurface (bool),
// captureBeyondViewport (bool, for full-page)
```

Full-page: set `captureBeyondViewport: true` + resize viewport first.

## PDF

```js
const { data } = await session.Page.printToPDF({
  landscape: false,
  printBackground: true,
  paperWidth: 8.27,   // inches (A4)
  paperHeight: 11.69,
  marginTop: 0.4,
  marginBottom: 0.4,
  marginLeft: 0.4,
  marginRight: 0.4,
  displayHeaderFooter: true,
  headerTemplate: "<span></span>",
  footerTemplate: "<span class=pageNumber></span>"
})
// data is base64-encoded PDF
```

## Scrolling

```js
// Scroll to element
await session.Runtime.evaluate({
  expression: "document.querySelector('selector').scrollIntoView({behavior:'instant',block:'center'})"
})

// Scroll by delta (pixels)
await session.Input.synthesizeScrollGesture({
  x: 100, y: 100, xDistance: 0, yDistance: -1000,  // negative = up
  xOverscroll: 0, yOverscroll: 0,
  preventFling: true, speed: 800
})

// Infinite scroll: loop scroll + check height
var prev = 0, cur = 1
while (cur > prev) {
  prev = cur
  await session.Input.synthesizeScrollGesture({ x: 500, y: 500, yDistance: -2000, speed: 2000 })
  cur = Number((await session.Runtime.evaluate({
    expression: "document.body.scrollHeight",
    returnByValue: true
  })).result.value)
}
```

## Shadow DOM

```js
// Via JS: shadowRoot is accessible when mode="open"
(await session.Runtime.evaluate({
  expression: "document.querySelector('selector').shadowRoot.querySelector('inner').textContent",
  returnByValue: true
})).result.value

// Via CDP: enable ShadowDOM in query
const { root } = await session.DOM.getDocument()
const { nodeId } = await session.DOM.querySelector({ nodeId: root.nodeId, selector: "my-component" })
// Get shadow root
const { nodeId: shadowId } = await session.DOM.describeNode({ nodeId, pierce: true })
// Now query inside shadow
await session.DOM.querySelector({ nodeId: shadowId, selector: ".inner" })
```

## Viewport / device emulation

```js
// Set viewport
await session.Emulation.setDeviceMetricsOverride({
  width: 390, height: 844,        // iPhone 14 Pro
  deviceScaleFactor: 3,
  mobile: true,
  screenWidth: 390, screenHeight: 844
})

// Set user agent
await session.Network.setUserAgentOverride({ userAgent: "Mozilla/5.0 (iPhone; ...)" })

// Media query override (dark mode)
await session.Emulation.setEmulatedMedia({ features: [{ name: "prefers-color-scheme", value: "dark" }] })

// Reset
await session.Emulation.clearDeviceMetricsOverride()
await session.Network.setUserAgentOverride({ userAgent: "" })
await session.Emulation.setEmulatedMedia({})
```
