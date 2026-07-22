#!/usr/bin/env bun
/**
 * quality.ts — Content quality validation + bot page detection
 *
 * All functions are PURE: same inputs → same outputs. No I/O, no side effects.
 * Operates on extracted text content, not DOM.
 *
 * DESIGN:
 *   - validateContent() runs the full quality gate server-side
 *   - detectBotPage() / detectPaywall() / detectCaptcha() / detectEmptyShell()
 *     are specialized classifiers composable by the caller
 *   - needsRecrawl() decides whether re-fetching with different params would help
 *   - qualityGateCode() generates the JS snippet embedded in browser templates
 *   - secondaryQualityCheck() is a post-extraction pass on already-cleaned content
 */

// ─── Types ───────────────────────────────────────────────────────────────

export interface QualityResult {
  ok: boolean; // true if content is usable by AI agents
  score: number; // 0-1 quality score
  reason?: string; // why it failed (if ok=false)
}

// ─── Shared Error Page Patterns ──────────────────────────────────────────
// These patterns detect browser error pages and empty content.
// Shared between server-side validateContent() and browser-side qualityGateCode().
const ERROR_PAGE_PATTERNS = [
  /This site can't be reached/i,
  /ERR_CONNECTION/i,
  /404 Not Found/i,
  /^\s*$/,
];

const ERROR_PAGE_REASONS: [RegExp, string][] = [
  [/This site can't be reached/i, 'browser_error_page'],
  [/ERR_CONNECTION/i, 'connection_error'],
  [/404 Not Found/i, 'not_found'],
  [/^\s*$/, 'whitespace_only'],
];

// ─── Content Validation ──────────────────────────────────────────────────

/**
 * validateContent: Main content validation gate.
 * Matches the same patterns as the browser-side quality gate (qualityGateCode)
 * but runs server-side on already-extracted text.
 *
 * Checks: readability, completeness, natural language signal.
 */
export function validateContent(text: string): QualityResult {
  if (!text || text.length === 0) {
    return { ok: false, score: 0, reason: 'empty_content' };
  }

  const slice = text.slice(0, 15000);
  let failures: string[] = [];
  let score = 1.0;

  // Too short to be useful (< 80 chars)
  if (slice.length < 80) {
    failures.push('too_short');
    score -= 0.4;
  }

  // Browser error pages / empty shell (shared patterns)
  for (const [pattern, reason] of ERROR_PAGE_REASONS) {
    if (pattern.test(slice)) {
      failures.push(reason);
      score -= 0.5;
    }
  }

  // Paywall teaser: "Read More »" as entire content
  if (slice === 'Read More \u00bb') {
    failures.push('paywall_teaser');
    score -= 0.5;
  }

  // Navigation chrome: short content with no natural language (no quotes)
  if (slice.length < 100 && slice.indexOf('"') < 0) {
    failures.push('navigation_chrome');
    score -= 0.3;
  }

  // Very few words suggests empty shell
  const wordCount = slice.split(/\s+/).length;
  if (wordCount < 15 && slice.length < 200) {
    failures.push('too_few_words');
    score -= 0.2;
  }

  // No punctuation suggests machine output
  if (slice.length > 100 && !/[.!?]/.test(slice)) {
    failures.push('no_punctuation');
    score -= 0.1;
  }

  // Bonus for natural language indicators
  if (/[.!?]/.test(slice)) score += 0.05;
  if (/\w{4,}/.test(slice)) score += 0.05;
  if (/\n\n/.test(slice)) score += 0.05;

  score = Math.max(0, Math.min(1, score));

  return {
    ok: score >= 0.5,
    score: Math.round(score * 100) / 100,
    reason: failures.length > 0 ? failures[0] : undefined,
  };
}

// ─── Bot Page Detection ──────────────────────────────────────────────────

/**
 * detectBotPage: Detect browser challenge / bot protection pages.
 * Patterns: Cloudflare, CAPTCHA, DataDome, browser integrity checks.
 */
export function detectBotPage(text: string, _url?: string): boolean {
  const patterns: RegExp[] = [
    /checking your browser/i,
    /cf-chl/,
    /turnstile/i,
    /verify you are human/i,
    /are you a human/i,
    /browser integrity check/i,
    /challenge-platform/i,
    /datadome/i,
    /just a moment\.\.\./i,
    /checking the browser/i,
    /cloudflare/i,
  ];
  return patterns.some((p) => p.test(text));
}

// ─── CAPTCHA Detection ───────────────────────────────────────────────────

/**
 * detectCaptcha: Detect CAPTCHA challenges.
 * Checks for reCAPTCHA iframes, Turnstile widgets, etc.
 */
export function detectCaptcha(text: string): boolean {
  const patterns: RegExp[] = [
    /captcha/i,
    /recaptcha/i,
    /g-recaptcha/i,
    /h-captcha/i,
    /turnstile/i,
    /cf-turnstile/i,
  ];
  return patterns.some((p) => p.test(text));
}

// ─── Paywall Detection ───────────────────────────────────────────────────

/**
 * detectPaywall: Detect paywall / subscription walls.
 * Looks for teaser text, login prompts, subscription enticements.
 */
export function detectPaywall(text: string): boolean {
  const patterns: RegExp[] = [
    /subscribe( to)? (for|to read|to continue|now)/i,
    /log in to read/i,
    /log in to continue/i,
    /sign in to read/i,
    /sign in to continue/i,
    /you've read your free articles/i,
    /this is a subscriber/i,
    /subscription required/i,
    /support our (journalism|newsroom)/i,
    /become a subscriber/i,
    /already a subscriber/i,
    /read the full article.*subscribe/i,
    /continue reading.*subscribe/i,
    /unlimited (access|digital) access/i,
    /paid (article|content)/i,
    /this article is (behind a|exclusively for)/i,
  ];
  return patterns.some((p) => p.test(text));
}

// ─── Empty Shell Detection ───────────────────────────────────────────────

/**
 * detectEmptyShell: Detect JS-required pages that render nothing useful.
 * These pages require JavaScript to render content but the extraction
 * captured only the empty shell.
 */
export function detectEmptyShell(text: string): boolean {
  // Too short to have meaningful content
  if (text.length < 80) return true;

  // JS-required message
  if (/please enable javascript/i.test(text)) return true;

  // Fewer than 10 words — likely navigation chrome only
  const words = text.split(/\s+/).filter(Boolean);
  if (words.length < 10) return true;

  return false;
}

// ─── Recrawl Decision ────────────────────────────────────────────────────

/**
 * needsRecrawl: Should this URL be re-crawled with different parameters?
 * Returns true if content quality is poor enough that a retry might help.
 */
export function needsRecrawl(result: QualityResult): boolean {
  // Very low quality — definitely retry
  if (result.score < 0.3) return true;

  // Borderline — retry if the reason suggests a params issue
  if (result.score < 0.5) {
    const retryableReasons = [
      'too_short',
      'too_few_words',
      'navigation_chrome',
    ];
    if (result.reason && retryableReasons.includes(result.reason)) return true;
  }

  return false;
}

// ─── Browser-Side Quality Gate (JS Code Generator) ──────────────────────

/**
 * qualityGateCode: Pure — returns JS snippet that checks content quality
 * inside the browser's V8 context.
 *
 * Sets `isJunk` and `loadError` in the enclosing scope.
 * Refers to `content` and `loadError` variables in enclosing scope.
 *
 * This is the canonical quality gate used by all extraction templates.
 * Keep in sync with validateContent() above.
 */
export function qualityGateCode(): string {
  return [
    'content=(content||"").slice(0,15000);',
    'isJunk=content.length<80||',
    ...ERROR_PAGE_PATTERNS.map((p) => '/' + p.source + '/' + p.flags + '.test(content)||'),
    'content==="Read More \\u00bb"||',
    '(content.length<100&&content.indexOf("\\"")<0);',
    'if(isJunk&&!loadError&&tab.type!=="pdf")loadError="low_quality_content";',
    'if(isJunk&&tab.type==="pdf"&&!loadError)loadError="pdf_textlayer_empty";',
  ].join('');
}

// ─── Secondary Quality Check ─────────────────────────────────────────────

/**
 * secondaryQualityCheck: Post-extraction validation on already-cleaned content.
 * More lenient than validateContent — used after the browser-side quality
 * gate has already filtered obvious junk.
 *
 * Checks for:
 *   - Truncation (content ends mid-sentence)
 *   - Error-page artifacts that passed the first gate
 *   - Content that's too short to be useful
 */
export function secondaryQualityCheck(text: string): QualityResult {
  if (!text || text.length === 0) {
    return { ok: false, score: 0, reason: 'empty_after_extraction' };
  }

  let score = 1.0;
  const failures: string[] = [];

  // Truncation detection: ends mid-sentence (last char is not sentence-ending)
  const trimmed = text.trim();
  if (trimmed.length > 100) {
    const lastChar = trimmed[trimmed.length - 1];
    const secondLast = trimmed[trimmed.length - 2];
    if (
      /[a-zA-Z0-9]/.test(lastChar) &&
      !/[.!?]/.test(secondLast + lastChar)
    ) {
      failures.push('truncated');
      score -= 0.1;
    }
  }

  // Still too short
  if (text.length < 80) {
    // If the browser gate said it's fine but we have < 80 chars, it's suspicious
    // This can happen if the page is a single short string (e.g., redirect confirmation)
    const suspiciousPatterns = [
      /redirect/i,
      /click here/i,
      /please continue|click to continue|continue to next/i,
      /loading/i,
      /please wait/i,
    ];
    if (suspiciousPatterns.some((p) => p.test(text))) {
      failures.push('suspicious_short');
      score -= 0.3;
    }
  }

  // Low word density suggests sparse content
  const words = text.split(/\s+/).filter(Boolean).length;
  if (words < 20 && text.length > 0) {
    failures.push('sparse_content');
    score -= 0.2;
  }

  // Check for repetitive content (same sentence appearing many times)
  const sentences = text.split(/[.!?]+/).filter((s) => s.trim().length > 20);
  if (sentences.length > 3) {
    const unique = new Set(sentences.map((s) => s.trim().toLowerCase()));
    if (unique.size < sentences.length * 0.5) {
      failures.push('repetitive');
      score -= 0.2;
    }
  }

  score = Math.max(0, Math.min(1, score));

  return {
    ok: score >= 0.5,
    score: Math.round(score * 100) / 100,
    reason: failures.length > 0 ? failures[0] : undefined,
  };
}
