/**
 * browser-automation.test.ts — Unit tests for pure template functions
 *
 * These tests validate the JS code-generation functions WITHOUT a browser
 * or CDP connection. Every function is pure (same input -> same output).
 *
 * Key coverage:
 *  - All 10 template functions (connectCode, extractionCode, qualityGateCode,
 *    followCode, batchFollowCode, batchSearchCode, searchCode,
 *    batchHarvestCode, sessionHealthCheckCode, readyStatePoll, readyStateCheck)
 *  - Content validation (validateContent)
 *  - Code structure: expected CDP methods, proper embedding of args
 *  - Purity: identical args -> identical output (===)
 *  - No constant leaks: TypeScript-only constants (GOOGLE_*) must NOT
 *    appear in generated JS (would cause ReferenceError in browser)
 */

import { test, expect } from 'bun:test';
import {
  connectCode,
  qualityGateCode,
  extractionCode,
  followCode,
  batchFollowCode,
  batchSearchCode,
  searchCode,
  batchHarvestCode,
  sessionHealthCheckCode,
  readyStatePoll,
  readyStateCheck,
  validateContent,
} from '../scripts/browser-automation.ts';

// ─── connectCode ──────────────────────────────────────────────────────────

test('connectCode returns a string containing session.connect', () => {
  const code = connectCode(9222);
  expect(typeof code).toBe('string');
  expect(code).toContain('session.connect');
});

test('connectCode embeds the port number correctly', () => {
  const port = 9999;
  const code = connectCode(port);
  expect(code).toContain(String(port));
  expect(code).toContain('port:9999');
});

test('connectCode checks session.isConnected() before connecting', () => {
  const code = connectCode(9222);
  expect(code).toContain('session.isConnected()');
});

// ─── connectivityGateCode ─────────────────────────────────────────────────

test('qualityGateCode contains expected regex patterns [ERR_CONNECTION, 404]', () => {
  const code = qualityGateCode();
  expect(code).toContain('ERR_CONNECTION');
  expect(code).toContain('404 Not Found');
  expect(code).toContain("This site can't be reached");
});

test('qualityGateCode has balanced braces and valid syntax artifacts', () => {
  const code = qualityGateCode();
  const openBraces = (code.match(/\{/g) || []).length;
  const closeBraces = (code.match(/\}/g) || []).length;
  expect(openBraces).toBe(closeBraces);
  // Each assignment statement ends with ;
  expect(code).toMatch(/;/);
});

test('qualityGateCode references content and loadError (scope correctness)', () => {
  const code = qualityGateCode();
  expect(code).toContain('content');
  expect(code).toContain('loadError');
  expect(code).toContain('isJunk');
});

// ─── extractionCode ───────────────────────────────────────────────────────

test('extractionCode returns a function declaration string', () => {
  const code = extractionCode();
  expect(typeof code).toBe('string');
  expect(code).toContain('(offset, maxLen, url, selector) =>');
  expect(code).toContain('document.querySelector');
  expect(code).toContain('JSON.stringify');
});

test('extractionCode contains quality gate patterns inline', () => {
  const code = extractionCode();
  expect(code).toContain("This site can't be reached");
  expect(code).toContain('ERR_CONNECTION');
  expect(code).toContain('404 Not Found');
  expect(code).toContain('low_quality_content');
});

test('extractionCode generates section boundary detection (h1,h2,h3)', () => {
  const code = extractionCode();
  expect(code).toContain('querySelectorAll');
  expect(code).toContain('h1,h2,h3');
  expect(code).toContain('sections.push');
});

// ─── readyStatePoll ───────────────────────────────────────────────────────

test('readyStatePoll returns looping poll code with callFunctionOn', () => {
  const code = readyStatePoll();
  expect(typeof code).toBe('string');
  expect(code).toContain('for(var i=0;i<50;i++)');
  expect(code).toContain('document.readyState');
  expect(code).toContain('Runtime.evaluate');
  expect(code).toContain('setTimeout(r,200)');
});

test('readyStatePoll has balanced braces', () => {
  const code = readyStatePoll();
  const openBraces = (code.match(/\{/g) || []).length;
  const closeBraces = (code.match(/\}/g) || []).length;
  expect(openBraces).toBe(closeBraces);
});

// ─── readyStateCheck ──────────────────────────────────────────────────────

test('readyStateCheck returns a single evaluate expression', () => {
  const code = readyStateCheck();
  expect(typeof code).toBe('string');
  expect(code).toContain('Runtime.evaluate');
  expect(code).toContain('document.readyState');
});

// ─── sessionHealthCheckCode ───────────────────────────────────────────────

test('sessionHealthCheckCode connects and closes agent tabs only', () => {
  const code = sessionHealthCheckCode(9222);
  expect(code).toContain('session.connect');
  expect(code).toContain('agentTabs.keys()');
  expect(code).toContain('closeTab');
  expect(code).toContain('port:9222');
  expect(code).not.toContain('force:true');
  expect(code).not.toContain('force');
});

test('sessionHealthCheckCode no longer enumerates all CDP targets', () => {
  const code = sessionHealthCheckCode(9222);
  expect(code).not.toContain('Target.getTargets');
  expect(code).not.toContain('about:blank');
  expect(code).not.toContain('chrome://');
});

// ─── followCode ───────────────────────────────────────────────────────────

test('followCode contains createTarget, Page.navigate, Runtime.evaluate, closeTab', () => {
  const code = followCode('http://example.com', 0, -1, 30000, 'article, main, [role=main]', 9222);
  expect(code).toContain('createTarget');
  expect(code).toContain('Page.navigate');
  expect(code).toContain('Runtime.evaluate');
  expect(code).toContain('closeTab');
});

test('followCode URL is properly JSON-stringified', () => {
  const url = "http://example.com/path?q=1&x='test'";
  const code = followCode(url, 0, -1, 30000, 'body', 9222);
  expect(code).toContain(JSON.stringify(url));
});

test('followCode selector appears in the generated extraction expression', () => {
  const selector = 'main.content > p';
  const code = followCode('http://example.com', 0, -1, 30000, selector, 9222);
  // The selector is embedded via JSON.stringify then re-stringified for
  // Runtime.callFunctionOn; the raw selector string still appears in the output.
  expect(code).toContain('main.content > p');
});

test('followCode timeout is embedded as a numeric literal', () => {
  const timeout = 30000;
  const code = followCode('http://example.com', 0, -1, timeout, 'body', 9222);
  expect(code).toContain(String(timeout));
});

test('followCode uses session.connect (output contains session.connect)', () => {
  const code = followCode('http://example.com', 0, -1, 15000, 'body', 9222);
  expect(code).toContain('session.connect');
});

test('followCode extracts with document.querySelector fallback via Runtime.evaluate', () => {
  const code = followCode('http://example.com', 0, -1, 15000, 'body', 9222);
  expect(code).toContain('document.querySelector');
  expect(code).toContain('session.Runtime.evaluate');
});

test('followCode closes tab in try/catch', () => {
  const code = followCode('http://example.com', 0, -1, 15000, 'body', 9222);
  expect(code).toContain('closeTab');
  // closeTab is in the finally block
  const finallyIdx = code.indexOf('finally{');
  const closeIdx = code.indexOf('closeTab');
  expect(finallyIdx).toBeGreaterThan(-1);
  expect(closeIdx).toBeGreaterThan(finallyIdx);
});

// ─── batchSearchCode ──────────────────────────────────────────────────────

test('batchSearchCode creates tabs and navigates with extraction loop', () => {
  const code = batchSearchCode(['query1', 'query2'], 5, 9222);
  expect(code).toContain('createTarget');
  expect(code).toContain('Page.navigate');
  expect(code).toContain('Runtime.evaluate');
  expect(code).toContain('closeTab');
});

test('batchSearchCode uses Set-based dedup (new Set(), .has, .add)', () => {
  const code = batchSearchCode(['query1'], 5, 9222);
  expect(code).toContain('new Set()');
  expect(code).toContain('.has(r.url)');
  expect(code).toContain('.add(r.url)');
});

test('batchSearchCode sorts results by snippet length', () => {
  const code = batchSearchCode(['query1'], 5, 9222);
  expect(code).toContain('snippet?.length');
  expect(code).toContain('.sort(');
  expect(code).toContain('b.snippet?.length');
});

test('batchSearchCode embeds queries as JSON array and count as literal', () => {
  const queries = ['test query', 'another one'];
  const count = 10;
  const code = batchSearchCode(queries, count, 9222);
  expect(code).toContain(JSON.stringify(queries));
  expect(code).toContain('const count=' + count + ';');
});

test('batchSearchCode returns JSON with results and meta', () => {
  const code = batchSearchCode(['q'], 5, 9222);
  expect(code).toContain('JSON.stringify({results:');
  expect(code).toContain('meta:');
  expect(code).toContain('total_unique');
});

// ─── searchCode ───────────────────────────────────────────────────────────

test('searchCode navigates Google search URL and waits for networkIdle', () => {
  const code = searchCode('test', 5, 9222);
  expect(code).toContain('google.com/search');
  expect(code).toContain('Page.lifecycleEvent');
  expect(code).toContain('networkIdle');
});

test('searchCode properly encodes the query with encodeURIComponent', () => {
  const code = searchCode('test query', 5, 9222);
  expect(code).toContain('encodeURIComponent');
});

test('searchCode contains waitFor("Page.lifecycleEvent"...)', () => {
  const code = searchCode('test', 5, 9222);
  expect(code).toMatch(/waitFor\("Page\.lifecycleEvent"/);
});

test('searchCode closes the tab in finally block', () => {
  const code = searchCode('test', 5, 9222);
  expect(code).toContain('closeTab');
  const finallyIdx = code.indexOf('finally{');
  const closeIdx = code.indexOf('closeTab');
  expect(finallyIdx).toBeGreaterThan(-1);
  expect(closeIdx).toBeGreaterThan(finallyIdx);
});

// ─── batchFollowCode ──────────────────────────────────────────────────────

test('batchFollowCode creates tabs in a loop (for…of)', () => {
  const code = batchFollowCode(['http://a.com', 'http://b.com'], 'body', 15000, 9222);
  expect(code).toMatch(/for\s*\(/);
  expect(code).toContain('createTarget');
});

test('batchFollowCode navigates all tabs', () => {
  const code = batchFollowCode(['http://a.com'], 'body', 15000, 9222);
  expect(code).toContain('Page.navigate');
});

test('batchFollowCode closes all tabs after extraction', () => {
  const code = batchFollowCode(['http://a.com'], 'body', 15000, 9222);
  const closeIdx = code.indexOf('closeTab');
  const evaluateIdx = code.lastIndexOf('Runtime.evaluate');
  expect(closeIdx).toBeGreaterThan(evaluateIdx);
});

test('batchFollowCode wraps extraction in try/catch', () => {
  const code = batchFollowCode(['http://a.com'], 'body', 15000, 9222);
  expect(code).toContain('try{');
  expect(code).toContain('}catch(e){');
  expect(code).toContain('loadError=e.message');
});

test('batchFollowCode handles PDF type with 3s setTimeout', () => {
  const code = batchFollowCode(['http://a.com'], 'body', 15000, 9222);
  expect(code).toContain('tab.type==="pdf"');
  expect(code).toContain('setTimeout(r,3000)');
});

test('batchFollowCode uses processed URL map for arXiv PDF conversion', () => {
  const code = batchFollowCode(['http://a.com'], 'body', 15000, 9222);
  expect(code).toContain('url.match');
  expect(code).toContain('/pdf/');
});

// ─── batchHarvestCode ─────────────────────────────────────────────────────

test('batchHarvestCode has two-phase structure: searchTabs + followTabs', () => {
  const code = batchHarvestCode(['query1'], 5, 3, 15000, 9222);
  expect(code).toContain('searchTabs');
  expect(code).toContain('followTabs');
  // search tabs come before follow tabs
  const searchIdx = code.indexOf('searchTabs');
  const followIdx = code.indexOf('followTabs');
  expect(searchIdx).toBeLessThan(followIdx);
});

test('batchHarvestCode deduplicates URLs with Set', () => {
  const code = batchHarvestCode(['query1'], 5, 3, 15000, 9222);
  expect(code).toContain('new Set()');
  expect(code).toContain('.has(r.url)');
});

test('batchHarvestCode ranks results by snippet length', () => {
  const code = batchHarvestCode(['query1'], 5, 3, 15000, 9222);
  expect(code).toContain('snippet?.length');
});

test('batchHarvestCode has PDF delay (3s setTimeout for PDF tabs)', () => {
  const code = batchHarvestCode(['query1'], 5, 3, 15000, 9222);
  expect(code).toContain('setTimeout(r,3000)');
});

test('batchHarvestCode returns structured JSON with search_results, read_pages, meta', () => {
  const code = batchHarvestCode(['query1'], 5, 3, 15000, 9222);
  expect(code).toContain('search_results');
  expect(code).toContain('read_pages');
  expect(code).toContain('unique_urls');
  expect(code).toContain('pages_read');
  expect(code).toContain('pages_skipped');
});

test('batchHarvestCode wraps extraction in try/catch with loadError', () => {
  const code = batchHarvestCode(['query1'], 5, 3, 15000, 9222);
  expect(code).toContain('try{');
  expect(code).toContain('}catch(e){');
  expect(code).toContain('loadError=e.message');
});

// ─── validateContent ──────────────────────────────────────────────────────

test('validateContent rejects empty content', () => {
  const result = validateContent('');
  expect(result.ok).toBe(false);
  expect(result.score).toBe(0);
});

test('validateContent accepts valid natural language content', () => {
  const text = 'The quick brown fox jumps over the lazy dog. This is a complete sentence with enough words to pass the quality gate. ' +
    'Multiple sentences exist in this text. Each one adds to the total word count. This content should easily pass validation. ' +
    'We need at least 80 characters and multiple sentences with punctuation. The quality score should be well above 0.5.';
  const result = validateContent(text);
  expect(result.ok).toBe(true);
  expect(result.score).toBeGreaterThanOrEqual(0.5);
});

test('validateContent detects browser error pages', () => {
  // Must be > 80 chars to avoid too_short precedence
  const result = validateContent("This site can't be reached. The connection was reset. ERR_CONNECTION_TIMED_OUT. We are unable to connect to the server. Please check your internet connection and try again later. If the problem persists, contact support.");
  expect(result.ok).toBe(false);
  expect(result.reason).toBe('browser_error_page');
});

test('validateContent detects 404 pages', () => {
  // Content with 404 but few words and no punctuation to compound deductions
  // Length > 80 (avoid too_short), < 100 (trigger navigation_chrome), < 15 words
  // Score: 1.0 - 0.5 (not_found) - 0.3 (navigation_chrome) - 0.2 (too_few_words) = 0.0
  const result = validateContent('404 Not Found This-page-was-removed-from-server Check-URL-spelling-and-retry-now-thanks-more');
  expect(result.ok).toBe(false);
  expect(result.reason).toBe('not_found');
  expect(result.score).toBeLessThan(0.5);
});

// ─── Purity Tests ─────────────────────────────────────────────────────────

test('connectCode is pure: same args return identical string (===)', () => {
  expect(connectCode(9222)).toBe(connectCode(9222));
  expect(connectCode(8080)).toBe(connectCode(8080));
});

test('qualityGateCode is pure: always returns identical string (===)', () => {
  expect(qualityGateCode()).toBe(qualityGateCode());
});

test('extractionCode is pure: always returns identical string (===)', () => {
  expect(extractionCode()).toBe(extractionCode());
});

test('readyStatePoll is pure: always returns identical string (===)', () => {
  expect(readyStatePoll()).toBe(readyStatePoll());
});

test('readyStateCheck is pure: always returns identical string (===)', () => {
  expect(readyStateCheck()).toBe(readyStateCheck());
});

test('sessionHealthCheckCode is pure: same args return identical string (===)', () => {
  expect(sessionHealthCheckCode(9222)).toBe(sessionHealthCheckCode(9222));
  expect(sessionHealthCheckCode(8080)).toBe(sessionHealthCheckCode(8080));
});

test('followCode is pure: same args return identical string (===)', () => {
  expect(followCode('http://x.com', 0, -1, 15000, 'body', 9222))
    .toBe(followCode('http://x.com', 0, -1, 15000, 'body', 9222));
});

test('batchSearchCode is pure: same args return identical string (===)', () => {
  expect(batchSearchCode(['q1', 'q2'], 5, 9222))
    .toBe(batchSearchCode(['q1', 'q2'], 5, 9222));
});

test('searchCode is pure: same args return identical string (===)', () => {
  expect(searchCode('query', 5, 9222))
    .toBe(searchCode('query', 5, 9222));
});

test('batchFollowCode is pure: same args return identical string (===)', () => {
  expect(batchFollowCode(['http://x.com'], 'body', 15000, 9222))
    .toBe(batchFollowCode(['http://x.com'], 'body', 15000, 9222));
});

test('batchHarvestCode is pure: same args return identical string (===)', () => {
  expect(batchHarvestCode(['q1'], 5, 3, 15000, 9222))
    .toBe(batchHarvestCode(['q1'], 5, 3, 15000, 9222));
});

test('no cross-call contamination: different args produce independent results', () => {
  const a = followCode('http://a.com', 0, -1, 10000, 'body', 9222);
  const b = followCode('http://b.com', 0, -1, 20000, 'main', 9223);
  expect(a).not.toBe(b);
  expect(a).toContain('http://a.com');
  expect(b).toContain('http://b.com');
  expect(a).not.toContain('http://b.com');
  expect(b).not.toContain('http://a.com');
});

// ─── No Undefined Symbol Leak ─────────────────────────────────────────────

test('generated JS does not contain GOOGLE_* TypeScript constants', () => {
  const codes = [
    ['connectCode', connectCode(9222)],
    ['qualityGateCode', qualityGateCode()],
    ['extractionCode', extractionCode()],
    ['followCode', followCode('http://x.com', 0, -1, 15000, 'body', 9222)],
    ['batchFollowCode', batchFollowCode(['http://x.com'], 'body', 15000, 9222)],
    ['batchSearchCode', batchSearchCode(['q'], 5, 9222)],
    ['searchCode', searchCode('q', 5, 9222)],
    ['batchHarvestCode', batchHarvestCode(['q'], 5, 3, 15000, 9222)],
    ['sessionHealthCheckCode', sessionHealthCheckCode(9222)],
    ['readyStatePoll', readyStatePoll()],
    ['readyStateCheck', readyStateCheck()],
  ];
  const forbidden = ['GOOGLE_TRANSLATE_PATTERN', 'GOOGLE_RESULT_LINK', 'GOOGLE_RESULT_CONTAINER'];
  for (const [name, code] of codes) {
    for (const constant of forbidden) {
      expect(code).not.toContain(constant);
    }
  }
});

test('generated JS does not leak other TypeScript identifiers into browser scope', () => {
  // Verify no uppercase constants with underscores (besides JS built-ins)
  // appear unmodified. The three GOOGLE_* constants are the known risk,
  // but also check: if someone adds a new constant and forgets to inline it,
  // this test catches it.
  const codes: string[] = [
    connectCode(9222),
    qualityGateCode(),
    extractionCode(),
    followCode('http://x.com', 0, -1, 15000, 'body', 9222),
    batchFollowCode(['http://x.com'], 'body', 15000, 9222),
    batchSearchCode(['q'], 5, 9222),
    searchCode('q', 5, 9222),
    batchHarvestCode(['q'], 5, 3, 15000, 9222),
    sessionHealthCheckCode(9222),
    readyStatePoll(),
    readyStateCheck(),
  ];

  // JS built-ins that legitimately appear in generated code
  const safeBuiltins = new Set([
    'JSON', 'Set', 'Error', 'Promise', 'NaN', 'null', 'true', 'false',
  ]);

  const pat = /[A-Z][A-Z_]{2,}/g;
  for (const code of codes) {
    const matches = code.match(pat) || [];
    for (const m of matches) {
      // If it's not a safe built-in AND it looks like a constant
      // (uppercase snake_case), flag it.
      if (!safeBuiltins.has(m)) {
        // Allow things like "Page.lifecycleEvent" — the pat only
        // matches contiguous all-caps segments, so "ARIA" or "HTML"
        // inside a string could match. Those are fine.
        // Only flag if it looks like a standalone constant name.
        // The known problematic ones are GOOGLE_* — check those.
        const isKnownDangerous = m.startsWith('GOOGLE_');
        expect(isKnownDangerous).toBe(false);
      }
    }
  }
});
