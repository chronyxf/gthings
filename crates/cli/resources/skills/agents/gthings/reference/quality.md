# Content Quality, Body Status, and Section Extraction

## BodyStatus (search --strategy harvest output)

Every harvested result has a `body_status` field for agent triage:

| Status | Meaning | Agent Action |
|--------|---------|-------------|
| `ok` | Full body extracted successfully | Use `followed_content` directly |
| `pdf_unextracted` | PDF/arXiv URL — CDP cannot extract | Fetch separately: `extract <url>` or `pdf-url <url>` |
| `extract_failed` | Paywall, bot wall, network error | Skip — no usable content |
| `chrome_or_empty` | CDP returned navigation chrome or empty text | Skip — nav-only or JS-rendered page |
| `snippet_only` | Only SERP snippet available, not followed | Use as search lead only, not body |

## QualityScore (present on extract/ax/search --strategy harvest)

Every extracted or harvested result includes `quality`:

```json
{
  "quality": {
    "score": 0.95,
    "is_ok": true,
    "reasons": [],
    "entropy_bits_per_char": 4.2
  }
}
```

### Score Interpretation

| Range | Meaning | Agent Decision |
|-------|---------|---------------|
| >= 0.80 | Clean, well-structured text | Use directly as factual source |
| 0.50 – 0.79 | Readable with minor issues | Use but verify key claims against other sources |
| < 0.50 | Low quality — artifacts, thin, or garbled | Do NOT use as primary source |

### reasons (non-empty when score < 0.8 or is_ok = false)

| Reason | Trigger | Agent Action |
|--------|---------|-------------|
| `empty_content` | Content is empty string | Skip completely |
| `bot_blocked` | Cloudflare/DataDome/Turnstile detected | Skip — cannot access |
| `paywall` | Subscription/paywall text detected | Skip — teaser only |
| `captcha` | reCAPTCHA/hCaptcha detected | Skip — blocked |
| `empty_shell` | JS-only page (< 80 chars) | Skip — no content |
| `too_short` | Fewer than 80 characters | Skip — insufficient |
| `too_few_words` | Fewer than 15 words | Skip — too thin |
| `low_entropy` | Shannon entropy < 2.0 bits/char | Skip — repetitive/garbled |

If `reasons` contains ANY of these, do NOT use the content as a factual source.

### entropy_bits_per_char

Shannon entropy of the extracted text. Useful for detecting:
- **Low entropy (< 2.0)**: Repetitive, thin, or machine-generated text
- **Normal (2.0 – 6.5)**: Natural language content
- **High (> 6.5)**: Potentially garbled or random text

## Quality Detection Functions

| Function | Detects | Sample Patterns |
|----------|---------|-----------------|
| `detect_bot` | Cloudflare, Turnstile, DataDome | "checking your browser", "just a moment", "cf-challenge" |
| `detect_captcha` | reCAPTCHA, hCaptcha | "recaptcha", "h-captcha", "cf-turnstile" |
| `detect_paywall` | Subscription prompts | "subscribe to read", "log in to continue", "subscribe now" |
| `detect_empty_shell` | JS-only / empty pages | "< 80 chars", "enable JavaScript" |

## Section Extraction

Every followed page has `sections` from heading detection:

```json
{
  "sections": [
    { "heading": "Introduction", "depth": 1, "offset": 0, "length": 500, "content": "Text...", "subsections": [] },
    { "heading": "Methods", "depth": 1, "offset": 500, "length": 2000, "content": "Text...", "subsections": [
      { "heading": "Statistical Approach", "depth": 2, "content": "..." }
    ]}
  ]
}
```

If `sections` is empty, use `followed_content` (harvest results) for full text.

## URL Canonicalization (for search --strategy harvest dedup)

Applied to every URL before dedup and ranking:
- Scheme+host lowercased
- Tracking params stripped: utm_*, fbclid, gclid, _ga, _gl, mc_cid, mc_eid
- Google text fragments removed: #:~:text=...
- All fragments stripped for dedup key
- Path lowercased, trailing slash stripped
- Query params sorted alphabetically
- Double slashes collapsed

## Secondary Quality Checks

| Check | Detects | Threshold |
|-------|---------|-----------|
| truncated | Content ends mid-sentence | Last char is alphanumeric, not .!? |
| repetitive | Same sentence repeated | Unique sentences < 50% of total |
| sparse | Very few words | < 20 words in text |

## Domain Reputation Cache

24-hour TTL cache per domain. After 2 consecutive BotWall or Paywall hits, domain is blocked from further follows. Clean extraction decays the counter.
