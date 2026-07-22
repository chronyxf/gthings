# PDF extraction

gsearch handles PDFs in three tiers, applied automatically depending on the URL.

## Tier 1: arXiv URL rewrite (automatic, instant)

When `batch follow` or `batch harvest` encounters a URL matching `arxiv.org/pdf/*`, the URL is rewritten to `arxiv.org/abs/*` **before a CDP tab is created**. No PDF is downloaded. No Chrome PDF viewer is loaded.

```
https://arxiv.org/pdf/2401.12345.pdf
  → https://arxiv.org/abs/2401.12345  (HTML abstract page)
```

The abstract page is regular HTML with title, authors, abstract, and references. CSS selectors work normally. This is the fastest and most reliable tier — it simply never loads a PDF.

## Tier 2: Chrome PDF viewer (automatic, best-effort)

For non-arXiv PDFs (any URL ending in `.pdf` or containing `/pdf/` in the path), the tab navigates to the URL and Chrome renders it using its built-in PDF viewer. After 3 seconds (to allow the text layer to render), the script tries to extract text:

```javascript
document.body?.innerText || document.querySelector("embed")?.shadowRoot?.textContent || ""
```

**Why 3 seconds?** Chrome's PDF viewer renders PDFs as canvas elements first, then overlays a text selection layer. The text layer takes time to appear. 3 seconds covers most PDFs under 10MB.

**When this fails:**
- Scanned/image-only PDFs have no text layer — extraction returns empty
- Large PDFs (>10MB) may need more than 3 seconds
- Encrypted PDFs are not supported at all
- PDFs with unusual encodings may produce garbled text

If extraction fails, the result includes `_error: "pdf_textlayer_empty"`.

## Tier 3: `gsearch pdftotext` (manual fallback)

When Chrome's PDF viewer can't extract text, use the dedicated PDF extraction command:

```bash
gsearch pdftotext "https://example.com/paper.pdf"
# → {"url":"...","content":"extracted text..."}
```

**How it works:**
1. Downloads the PDF via `curl` (30 second timeout)
2. Passes the raw bytes to `scripts/pdf-extract.ts`
3. The script scans the PDF for stream objects, decompresses FlateDecode streams using Node.js built-in `zlib`
4. Extracts text between parentheses `(text)` in PDF content stream operators
5. Joins all text chunks and outputs the result

**Limitations:**
- Only works on text-based PDFs (not scanned documents)
- PDFs with non-standard compression (LZW, JPEG2000) are not supported
- Table structure and formatting are lost — only raw text is extracted
- Depends on `curl` being available on the system (default on macOS, most Linux)

## URL detection logic

The tool decides how to handle a URL based on these rules, checked in order:

1. URL matches `arxiv.org/pdf/*` → Tier 1 (rewrite to abstract)
2. URL ends with `.pdf` (case-insensitive) → Tier 2 (Chrome viewer)
3. URL contains `/pdf/` in the path → Tier 2 (Chrome viewer)
4. Everything else → normal HTML extraction

## Error reference

| Error | When it appears | What's happening | Action |
|-------|----------------|------------------|--------|
| `pdf_textlayer_empty` | Tier 2 extraction | Chrome viewer rendered but `document.body.innerText` returned under 80 chars | Use `gsearch pdftotext <url>` |
| `pdf_extraction_failed: <msg>` | Tier 2 extraction | JS threw during `Runtime.evaluate` on the PDF tab | Try `gsearch pdftotext <url>`. If persists, PDF may be encrypted or corrupted. |
| No error, empty content | Any tier | URL loaded but returned no text | Check if URL is really a PDF. Some servers redirect PDF requests to HTML pages. |
