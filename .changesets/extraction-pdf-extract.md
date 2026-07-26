---
"gthings-extraction": patch
---

- Replace pdftotext CLI (poppler-utils) with pdf-extract (bundled MuPDF) for PDF text extraction
- Fix TeX font extraction failure (Computer Modern fonts in arxiv papers)
- Zero external dependencies — MuPDF builds from source via cargo
