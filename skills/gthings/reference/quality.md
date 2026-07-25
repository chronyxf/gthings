# Content Quality Gate and Section Extraction

## Status: Internal to Extraction Crate

The quality gate and section extraction now live **inside** the `gthings_extraction` crate and are **no longer surfaced via the CLI output**. The `FollowResult` returned by `gthings follow` does NOT include `quality` or `sections` fields.

However, the quality heuristics remain useful for **manual inspection** of `FollowResult.content` when you want to assess whether the extracted text is worthwhile before passing it to an LLM.

---

## Quality Gate (Reference for Manual Use)

### Pass Criteria (score >= 0.5)

Score starts at 1.0 with deductions:

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

### Fail Indicators

| Indicator | Meaning | Recovery |
|-----------|---------|----------|
| `too_short` (< 80 chars) | Barely any content | Retry with larger `--max-chars` |
| `browser_error_page` | Unreachable page | Verify URL |
| `paywall_teaser` | Subscription required | Try alternative source |
| `too_few_words` + `navigation_chrome` | Sign-in wall | Auth required |

### Bot/Captcha/Paywall Detection Patterns

| Pattern | Detects | Keywords |
|---------|---------|----------|
| Bot check | Cloudflare, DataDome, Turnstile | "checking your browser", "just a moment" |
| Captcha | reCAPTCHA, hCaptcha | "recaptcha", "h-captcha", "cf-turnstile" |
| Paywall | Subscription prompts | "subscribe to read", "log in to continue" |
| Empty shell | JS-only pages | < 80 chars, "enable JavaScript" |

## Section Extraction (Reference)

The extraction crate extracts sections from h1/h2/h3 elements but this data is not included in the CLI `FollowResult`. If you need structured sections, use the `gthings_extraction` crate directly.

### Method (for reference)

1. Extract via `document.body.innerText` (or CSS selector)
2. Find h1/h2/h3 via `querySelectorAll('h1,h2,h3')`
3. Collect sibling text until next heading
4. Content between headings = section content

### Secondary Checks for `FollowResult.content`

| Check | Detects | Threshold |
|-------|---------|-----------|
| `truncated` | Content ends mid-sentence | Last char is alphanumeric, not .!? |
| Repetitive text | Same sentence repeated | Unique sentences < 50% |
| Sparse content | Very few words | < 20 words |
| Suspicious short | Short + redirect keywords | < 80 chars + "redirect"/"click here" |
