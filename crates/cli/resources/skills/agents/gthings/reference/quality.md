# Content Quality Gate and Section Extraction

## Quality Gate

Every `follow` result includes `quality`:
```json
{"quality": {"score": 0.0-1.0, "is_ok": true, "reasons": [], "length": 45320}}
```

### Pass Criteria (`is_ok: true`)

Content passes when `score >= 0.5`. Score starts at 1.0 with deductions:

| Condition | Deduction | Reason |
|-----------|-----------|--------|
| Content < 80 chars | -0.4 | `too_short` |
| Browser error page | -0.5 | `browser_error_page` |
| Connection error | -0.5 | `connection_error` |
| 404 Not Found | -0.5 | `not_found` |
| Whitespace only | -0.5 | `whitespace_only` |
| Paywall teaser ("Read More »") | -0.5 | `paywall_teaser` |
| Short + no quotes | -0.3 | `navigation_chrome` |
| < 15 words + < 200 chars | -0.2 | `too_few_words` |
| > 100 chars, no punctuation | -0.1 | `no_punctuation` |

### Fail Criteria (`is_ok: false`)

| reasons | Meaning | Recovery |
|---------|---------|----------|
| `["too_short"]` | < 80 chars | `--selector "body" --max 30000` |
| `["browser_error_page"]` | Unreachable | Verify URL |
| `["paywall_teaser"]` | Subscription | Try alternative source |
| `["too_few_words", "navigation_chrome"]` | Sign-in required | Auth required |
| `["empty_content"]` | JS-rendered | Needs different approach |

### Bot/Captcha/Paywall Detection

| Function | Detects | Patterns |
|----------|---------|----------|
| `detect_bot` | Cloudflare, DataDome, Turnstile | "checking your browser", "just a moment" |
| `detect_captcha` | reCAPTCHA, hCaptcha | "recaptcha", "h-captcha", "cf-turnstile" |
| `detect_paywall` | Subscription prompts | "subscribe to read", "log in to continue" |
| `detect_empty_shell` | JS-only pages | < 80 chars, "enable JavaScript" |

### Retry Logic

`follow` auto-retries with `--selector "body" --timeout 30000` when `score < 0.3` (unconditionally) or `score < 0.5` AND reasons include `too_short`, `too_few_words`, or `navigation_chrome`.

## Section Extraction

Sections from h1/h2/h3 elements:
```json
{"sections": [{"heading": "Financial Report", "content": "..."}, {"heading": "Interest Rates", "content": "..."}]}
```

### Method
1. Extract via `document.body.innerText` (or CSS selector)
2. Find h1/h2/h3 via `querySelectorAll('h1,h2,h3')`
3. Collect sibling text until next heading
4. `content` = text between this heading and next

### Empty Sections

`sections: []` means no headings found. The `content` field still has full text.

### Secondary Quality Check

| Check | Detects | Threshold |
|-------|---------|-----------|
| `truncated` | Content ends mid-sentence | Last char is alphanumeric, not .!? |
| `repetitive` | Same sentence repeated | Unique sentences < 50% |
| `sparse` | Very few words | < 20 words |
| `suspicious_short` | Short + redirect keywords | < 80 chars + "redirect"/"click here" |
