# Content quality gate

When extracting text from web pages, not all content is equal. Some pages load successfully but return practically nothing useful — paywalls, error pages, login walls, or empty shells. The content quality gate detects these and marks them so you don't waste time processing junk.

## What gets filtered

**Empty or near-empty pages:** Pages where `document.body.innerText` returns under 80 characters. This catches most login walls, "Please enable JavaScript" messages, and blank pages.

**Connection error pages:** Browsers render their own error pages for DNS failures, connection refused, and SSL errors. These pages contain text like "This site can't be reached" or "ERR_CONNECTION_CLOSED" but zero useful content.

**Paywall teasers:** Many news sites return a teaser paragraph and "Read More »" or "Subscribe to continue" — all the navigation chrome but only a sentence of actual article content.

**Redirect loops:** Pages that immediately redirect to a login page or another domain often end up with very little content.

## Detection patterns

The gate checks extracted text against these patterns:

```
content.length < 80                     → too short to be useful
"This site can't be reached"            → browser error page
"ERR_CONNECTION"                        → connection error
"404 Not Found"                         → missing page
/^\s*$/                                 → whitespace only
"Read More »"                           → paywall teaser
content.length < 100 and no quotes      → likely navigation chrome
```

If any pattern matches, the page is marked with `_error: "low_quality_content"` and `content: ""`.

## How to use the result

When you see `_error: "low_quality_content"` in batch results, it means the page loaded but returned nothing useful. Try:

1. A different URL covering the same topic
2. A different source (Reuters vs CNBC vs Bloomberg)
3. For paywalled sites, try the article via a different route (e.g., Google search result directly)

The `meta.pages_skipped` field in batch harvest results tells you how many pages were filtered out. If this is consistently high, your source selection needs improvement.

## Implementing in your own scripts

If you're using `browser-harness-js` directly (not through gsearch), apply the same check after any text extraction:

```javascript
const content = (r.result?.value || "").slice(0, 15000);
const isJunk = content.length < 80 ||
  /This site can't be reached/i.test(content) ||
  /ERR_CONNECTION/i.test(content) ||
  /404 Not Found/i.test(content) ||
  /^\s*$/.test(content) ||
  content === "Read More »" ||
  (content.length < 100 && content.indexOf('"') < 0);

if (isJunk) {
  // Mark as low quality — don't include as valid content
  results.push({url: tab.url, content: "", _error: "low_quality_content"});
} else {
  results.push({url: tab.url, content});
}
```
