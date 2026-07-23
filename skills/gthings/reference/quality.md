# Content Quality Gate and Section Extraction

## Quality Gate

Every `follow` result includes a `quality` object:

```json
{
  "quality": {
    "score": 0.0-1.0,
    "is_ok": true,
    "reasons": [],
    "length": 45320
  }
}
```

### Pass Criteria (`is_ok: true`)

Content passes when `score >= 0.5`. Score starts at 1.0 and gets deductions:

| Condition | Deduction | Reason string |
|-----------|-----------|---------------|
| Content < 80 chars | -0.4 | `too_short` |
| Browser error page | -0.5 | `browser_error_page` |
| Connection error | -0.5 | `connection_error` |
| 404 Not Found | -0.5 | `not_found` |
| Whitespace only | -0.5 | `whitespace_only` |
| Paywall teaser ("Read More »") | -0.5 | `paywall_teaser` |
| Short + no quotes ("") | -0.3 | `navigation_chrome` |
| < 15 words + < 200 chars | -0.2 | `too_few_words` |
| > 100 chars, no punctuation | -0.1 | `no_punctuation` |

### Fail Criteria (`is_ok: false`)

| `reasons` | Meaning | What to do |
|-----------|---------|------------|
| `["too_short"]` | Page had < 80 chars of text | Try `--selector "body" --max 30000` |
| `["browser_error_page"]` | "This site can't be reached" | URL is unreachable |
| `["paywall_teaser"]` | "Read More »" only | Subscription required |
| `["too_few_words", "navigation_chrome"]` | Sign-in page, nav only | Page requires authentication |
| `["empty_content"]` | No text extracted | Page is JS-rendered, needs different approach |

### Bot/Captcha/Paywall Detection

| Function | Detects | Patterns |
|----------|---------|----------|
| `detect_bot` | Cloudflare, DataDome, Turnstile | "checking your browser", "just a moment" |
| `detect_captcha` | reCAPTCHA, hCaptcha | "recaptcha", "h-captcha", "cf-turnstile" |
| `detect_paywall` | Subscription prompts | "subscribe to read", "log in to continue" |
| `detect_empty_shell` | JS-only pages | < 80 chars, "enable JavaScript" |

### Retry Logic

The `follow` command auto-retries with `--selector "body" --timeout 30000` when:
- `score < 0.3` (unconditionally)
- `score < 0.5` AND reasons include `too_short`, `too_few_words`, or `navigation_chrome`

## Section Extraction

Sections are extracted from h1/h2/h3 elements on the page:

```json
{
  "sections": [
    {"heading": "Financial Report 2026", "content": "Market overview content here."},
    {"heading": "Interest Rates", "content": "The Fed held rates at 3.50-3.75%."},
    {"heading": "Inflation Outlook", "content": "PCE inflation at 4.1%."}
  ]
}
```

### Extraction Method

1. Page text is extracted via `document.body.innerText` (or CSS selector if specified)
2. h1, h2, h3 elements are found via `querySelectorAll('h1,h2,h3')`
3. For each heading, sibling elements are collected until the next heading
4. `content` is the text between this heading and the next

### Empty Sections

`sections: []` means no h1/h2/h3 were found. The `content` field still has the full extracted text. Sections are an enhancement, not a guarantee.

### Secondary Quality Check

| Check | Detects | Threshold |
|-------|---------|-----------|
| `truncated` | Content ends mid-sentence | Last char is alphanumeric, not .!? |
| `repetitive` | Same sentence repeated | Unique sentences < 50% of total |
| `sparse` | Very few words | < 20 words |
| `suspicious_short` | Very short + redirect keywords | < 80 chars + "redirect"/"click here" |
