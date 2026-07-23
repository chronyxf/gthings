# Error Handling and Troubleshooting

## Exit Codes

| Code | Meaning | Common causes |
|------|---------|---------------|
| 0 | Success | — |
| 1 | Operational error | Bad URL, empty results, PDF parse failure, Chrome launch failure |

## Error Messages

### "PDF URL '...' returned HTTP 404 Not Found"

The PDF URL doesn't exist. Common cause: agent constructed wrong URL path.

```
→ Verify the URL in a browser
→ For arXiv: use the abstract page (arxiv.org/abs/XXXX.XXXXX), not a guessed PDF path
→ For IMF/World Bank: the actual PDF URL is different from the HTML page URL
```

### "URL '...' returned content type 'text/html' but does not appear to be a PDF"

The URL points to an HTML page, not a PDF file.

```
→ Use gthings --json follow url "<url>" --max 50000 instead of pdf url
→ The HTML page may contain the text you need
```

### "Failed to extract text from PDF at '...'"

The PDF is valid but extraction failed (unsupported features).

```
→ Common cause: PDF uses unsupported compression (LZW, ASCIIHexDecode, etc.)
→ Use gthings --json follow url "<url>" for the abstract/HTML version
```

### Browser not working

```
→ Check if browser is running: gthings browser status
→ Start it: gthings browser start
→ Stop and restart: gthings browser stop; gthings browser start
→ Check port usage: lsof -i :9222
→ Common: no Chrome/Dia installed, or another process is using port 9222
```

## Search Issues

### Empty search results

Google returned no results for the query.

```
→ The command auto-retries with trailing-space query
→ Try different wording
→ Check if browser is connected to the internet
→ Google may be showing a sign-in wall (common with automated browsers)
```

### Search returns sign-in pages

Google is showing a login wall because it detected automation.

```
→ This is expected with headless Chrome
→ The results still have titles and URLs — use them
→ Consider using a different search approach for the specific domain
```

## Follow Issues

### Content is truncated

The `truncated: true` flag means the page has more content.

```
→ Increase --max: gthings --json follow url "<url>" --max 50000
→ Use --offset to read next chunk: gthings --json follow url "<url>" --offset 15000 --max 15000
```

### Sections are empty

No h1/h2/h3 found on the page.

```
→ Check the content field — it still has the full text
→ Some pages use div-based layouts instead of semantic HTML
→ Not a bug — sections are best-effort
```

### Quality gate fails

`quality.is_ok: false`

```
→ Retry with --selector "body" --max 30000 --offset 0
→ If still failing, the page genuinely has no useful text content
→ Possible: paywall, captcha, JS-required page, error page
```

## PDF Issues

### PDF extraction returns empty text

The PDF was parsed but no text was found.

```
→ Common for scanned PDFs (images, not text)
→ Common for PDFs with unsupported font encodings
→ Try the HTML/abstract version of the paper
```

### "Failed to fetch PDF URL"

Network error during PDF download.

```
→ Check internet connectivity
→ The URL may require authentication
→ Try a different mirror or the abstract page
```

## Trace Issues

### --trace doesn't write to file

The trace file wasn't created.

```
→ The --trace flag only writes for actual commands (not --help or --version which exit via clap)
→ Use a real command: gthings --trace /tmp/t.jsonl --json browser status
→ Check write permissions on the trace path
```
