# Network & data: cookies · interception · downloads · uploads

> Before reaching for these low-level CDP snippets, check if
> `browser-automation.ts` commands (`follow`, `search`, `batch-follow`,
> `batch-search`, `batch-harvest`) already cover your use case. Use these
> interaction skills only when the high-level commands don't support
> what you need — for example, custom click sequences, multi-step forms,
> or intercepting network requests.

## Cookies

```js
// Get all cookies
const { cookies } = await session.Storage.getCookies({ browserContextId: "" })
// cookies = [{name,value,domain,path,secure,httpOnly,...}]

// Get cookies for a URL
const { cookies } = await session.Network.getCookies({ urls: ["https://example.com"] })

// Set cookie
await session.Network.setCookie({
  name: "session", value: "abc123",
  domain: ".example.com", path: "/",
  secure: true, httpOnly: true
})

// Delete cookie
await session.Network.deleteCookies({ name: "session", domain: ".example.com" })

// Clear all
await session.Storage.clearCookies({ browserContextId: "" })
```

## Network interception

```js
// Enable network tracking
await session.Network.enable()

// Block requests matching patterns
await session.Network.setBlockedURLs({ urls: ["*analytics*", "*tracking*"] })

// Subscribe to events
session.onEvent((method, params) => {
  if (method === "Network.requestWillBeSent") {
    // params.request.url, params.request.method, params.request.headers
  }
  if (method === "Network.responseReceived") {
    // params.response.url, params.response.status, params.response.headers
  }
  if (method === "Network.loadingFailed") {
    // params.errorText — why a request failed
  }
})

// Get response body
session.onEvent((method, params, sid) => {
  if (method === "Network.responseReceived") {
    const body = await session.Network.getResponseBody({ requestId: params.requestId })
    // body.body (text), body.base64Encoded (bool)
  }
})

// Intercept and modify requests (Request Interception pattern)
await session.Network.setRequestInterception({ patterns: [{ urlPattern: "*", interceptionStage: "HeadersReceived" }] })
session.onEvent((method, params) => {
  if (method === "Network.requestIntercepted") {
    // params.interceptionId, params.request.url
    // Continue, block, or modify
    session.Network.continueInterceptedRequest({ interceptionId: params.interceptionId })
  }
})
```

## Downloads

```js
// Enable download events
await session.Browser.setDownloadBehavior({
  behavior: "allow", downloadPath: "/tmp/downloads"
})

// Subscribe
session.onEvent((method, params) => {
  if (method === "Browser.downloadProgress") {
    // params.url, params.state ("inProgress"|"completed"|"canceled"),
    // params.totalBytes, params.receivedBytes
  }
})

// Trigger download (e.g. click download button)
await session.Runtime.evaluate({
  expression: "document.querySelector('.download-btn').click()"
})

// Wait for download to complete (poll via event)
// Event fires "completed" when done
```

## Upload file

```js
// Requires an <input type="file"> on the page

// Method 1: setFileInputFiles (recommended)
const { root } = await session.DOM.getDocument()
const { nodeId } = await session.DOM.querySelector({
  nodeId: root.nodeId, selector: "input[type=file]"
})
await session.DOM.setFileInputFiles({
  nodeId,
  files: ["/path/to/document.pdf"]
})

// Multiple files:
await session.DOM.setFileInputFiles({
  nodeId,
  files: ["/path/to/file1.pdf", "/path/to/file2.pdf"]
})
```
