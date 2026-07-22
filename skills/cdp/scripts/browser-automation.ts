#!/usr/bin/env bun
/**
 * browser-automation.ts — Thin CLI entry for CDP browser automation
 *
 * This is the CLI entry point. It imports pure template functions from
 * templates.ts, disk cache from cache.ts, and content quality validation
 * from quality.ts. It parses CLI args, dispatches to command handlers,
 * manages caching, and validates output quality.
 *
 * DESIGN:
 *   - THIN: ~180 lines of orchestration, no business logic
 *   - BACKWARD COMPATIBLE: same subcommands, flags, and JSON output format
 *   - QUALITY GATE: validates content after extraction, triggers recrawl
 *     if --retry flag is set and content fails quality checks
 *   - CACHE FIRST: checks disk cache before executing browser operations
 *
 * CLI:
 *   browser-automation.ts <command> [args...]
 *
 * Commands:
 *   follow <url> [--selector S] [--offset N] [--max M] [--timeout MS]
 *               [--port N] [--raw] [--pretty] [--retry]
 *   batch-follow <url1> ... [--selector S] [--offset N] [--max M]
 *                           [--timeout MS] [--port N]
 *   batch-harvest <q1> <q2> ... [--count N] [--max M] [--timeout MS]
 *                               [--port N]
 *   search <query> [--count N] [--port N]
 *   batch-search <q1> <q2> ... [--count N] [--port N]
 */

import { spawnSync } from 'node:child_process';

import {
  connectCode,
  followCode,
  extractionCode,
  batchFollowCode,
  searchCode,
  batchSearchCode,
  batchHarvestCode,
  sessionHealthCheckCode,
} from './templates';

import { cacheGetByUrl, cacheSetByUrl } from './cache';
import { validateContent, needsRecrawl, secondaryQualityCheck as secondaryCheck } from './quality';

// ─── Types ───────────────────────────────────────────────────────────────

export type Command =
  | 'follow'
  | 'batch-follow'
  | 'batch-harvest'
  | 'search'
  | 'batch-search';

export interface ParsedArgs {
  command: Command;
  positional: string[];
  selector: string;
  timeout: number;
  count: number;
  maxPages: number;
  port: number;
  raw: boolean;
  offset: number;
  maxLen: number;
  pretty: boolean;
  // [CLEANED] removed dead stream flag
  retry: boolean;
}

// ─── Subprocess Execution ────────────────────────────────────────────────

function runBrowserHarness(
  jsCode: string,
): { stdout: string; stderr: string; exitCode: number } {
  const timeoutMs = 60000; // 60s max
  try {
    const result = spawnSync('browser-harness-js', [], {
      input: jsCode,
      timeout: timeoutMs,
      maxBuffer: 10 * 1024 * 1024, // 10MB max output
      encoding: 'utf8',
      env: { ...process.env },
    });
    return {
      stdout: (result.stdout || '').trim(),
      stderr: (result.stderr || '').trim(),
      exitCode: result.status ?? -1,
    };
  } catch (e: any) {
    return {
      stdout: '',
      stderr: `runBrowserHarness error: ${e.message}`,
      exitCode: -1,
    };
  }
}

// ─── Session Health Check ────────────────────────────────────────────────

/** runHealthCheck: Execute session health check synchronously. */
function runHealthCheck(port: number): void {
  try {
    const js = sessionHealthCheckCode(port);
    spawnSync('browser-harness-js', [], {
      input: js,
      timeout: 10000,
      encoding: 'utf8',
    });
  } catch {
    // Health check failure is non-fatal
  }
}

// ─── CLI Parsing ─────────────────────────────────────────────────────────

function usage(): never {
  const msg = [
    'Usage:',
    '  browser-automation.ts follow <url> [--selector S] [--offset N] [--max M] [--timeout MS] [--port N] [--raw] [--pretty] [--retry]',
    '  browser-automation.ts batch-follow <url1> ... [--selector S] [--offset N] [--max M] [--timeout MS] [--port N]',
    '  browser-automation.ts batch-harvest <q1> ... [--count N] [--max M] [--timeout MS] [--port N]',
    '  browser-automation.ts search <query> [--count N] [--port N]',
    '  browser-automation.ts batch-search <q1> ... [--count N] [--port N]',
  ].join('\n');
  console.error(msg);
  process.exit(1);
}

export function parseArgs(argv: string[]): ParsedArgs {
  const cmd = argv[0] as Command;
  if (!cmd) usage();

  const validCommands: Command[] = [
    'follow',
    'batch-follow',
    'batch-harvest',
    'search',
    'batch-search',
  ];
  if (!(validCommands as string[]).includes(cmd)) usage();

  const args: ParsedArgs = {
    command: cmd,
    positional: [],
    selector: 'article, main, [role=main]',
    timeout: 15000,
    count: 5,
    maxPages: 5,
    port: 9222,
    raw: false,
    offset: 0,
    maxLen: 15000,
    pretty: false,
    // [CLEANED] removed dead stream flag
    retry: false,
  };

  let i = 1;
  while (i < argv.length) {
    const arg = argv[i];
    if (arg === '--selector' && i + 1 < argv.length) {
      args.selector = argv[i + 1];
      i += 2;
    } else if (arg === '--timeout' && i + 1 < argv.length) {
      args.timeout = parseInt(argv[i + 1], 10);
      i += 2;
    } else if (arg === '--count' && i + 1 < argv.length) {
      args.count = parseInt(argv[i + 1], 10);
      i += 2;
    } else if (arg === '--port' && i + 1 < argv.length) {
      args.port = parseInt(argv[i + 1], 10);
      i += 2;
    } else if (arg === '--offset' && i + 1 < argv.length) {
      args.offset = parseInt(argv[i + 1], 10);
      i += 2;
    } else if (arg === '--max' && i + 1 < argv.length) {
      const val = parseInt(argv[i + 1], 10);
      args.maxPages = val;
      args.maxLen = val;
      i += 2;
    } else if (arg === '--pretty') {
      args.pretty = true;
      i += 1;
    } else if (arg === '--raw') {
      args.raw = true;
      i += 1;
    } // [CLEANED] removed dead stream flag (--stream arg)
    else if (arg === '--retry') {
      args.retry = true;
      i += 1;
    } else if (arg.startsWith('--')) {
      console.error('Unknown option: ' + arg);
      usage();
    } else {
      args.positional.push(arg);
      i += 1;
    }
  }

  if (args.positional.length === 0) usage();
  return args;
}

// ─── Quality Helpers ─────────────────────────────────────────────────────

/**
 * Extract JSON result from stdout, validate content quality.
 * Returns { parsed, quality } or null if parsing fails.
 */
function validateOutput(
  stdout: string,
): { content: string; quality: ReturnType<typeof validateContent> } | null {
  try {
    const parsed = JSON.parse(stdout);
    const content = parsed.content || parsed.data || '';
    const quality = validateContent(content);
    return { content, quality };
  } catch {
    return null;
  }
}

/**
 * Print output in the appropriate format (raw, pretty, or wrapped).
 */
function printOutput(
  output: string,
  raw: boolean,
  pretty: boolean,
  success?: boolean,
  url?: string,
): void {
  if (pretty) {
    try {
      output = JSON.stringify(JSON.parse(output), null, 2);
    } catch {
      // Not valid JSON — output as-is
    }
  }

  if (raw) {
    console.log(output);
  } else if (success !== undefined && url !== undefined) {
    console.log(JSON.stringify({ success, url, data: output }));
  } else {
    console.log(output);
  }
}

// ─── Command Handlers ────────────────────────────────────────────────────

function cmdFollow(args: ParsedArgs): void {
  // arXiv normalization
  let url = args.positional[0];
  const pdfMatch = url.match(/(\/pdf\/|\.pdf$)/i);
  if (pdfMatch && pdfMatch[1] === '/pdf/') {
    url = url.replace(/\/pdf\/(.+)/, '/abs/$1').replace(/\.pdf$/, '');
  }

  // ★ Cache check
  const cached = cacheGetByUrl(url, args.offset, args.maxLen);
  if (cached) {
    printOutput(cached, args.raw, args.pretty, true, url);
    return;
  }

  // ★ Fetch via browser
  const js = followCode(
    url,
    args.offset,
    args.maxLen,
    args.timeout,
    args.selector,
    args.port,
  );
  const { stdout, stderr, exitCode } = runBrowserHarness(js);

  if (exitCode !== 0) {
    if (stdout) {
      printOutput(stdout, args.raw, args.pretty);
    } else {
      printOutput(
        JSON.stringify({ content: '', total_length: 0, _error: stderr?.trim?.() || 'follow_failed' }),
        args.raw,
        args.pretty,
      );
    }
    return;
  }

  // ★ Quality validation + optional retry
  let resultOutput = stdout;
  const parsed = validateOutput(stdout);
  if (parsed && args.retry) {
    // Run both primary and secondary quality checks
    const primary = parsed.quality;
    const secondary = secondaryCheck(parsed.content);

    if (!primary.ok && needsRecrawl(primary)) {
      // Retry with fallback selector + longer timeout
      const retryJs = followCode(
        url,
        args.offset,
        args.maxLen,
        Math.max(args.timeout, 30000),
        'body',
        args.port,
      );
      const retryResult = runBrowserHarness(retryJs);

      if (retryResult.exitCode === 0) {
        const retryParsed = validateOutput(retryResult.stdout);
        if (retryParsed) {
          const retryPrimary = retryParsed.quality;
          const retrySecondary = secondaryCheck(retryParsed.content);
          if (retryPrimary.ok && retrySecondary.ok) {
            // Retry succeeded with good quality — use retry result
            resultOutput = retryResult.stdout;
          }
        }
      }
      // If retry also fails, keep original result
    } else if (!primary.ok && !secondary.ok) {
      // Both checks failed and not retryable — mark with quality warning
      try {
        const parsedObj = JSON.parse(resultOutput);
        parsedObj._quality_warning = primary.reason || 'low_quality';
        resultOutput = JSON.stringify(parsedObj);
      } catch { /* keep as-is */ }
    }
  }

  // ★ Save to cache
  cacheSetByUrl(url, resultOutput, args.offset, args.maxLen);

  // ★ Output
  printOutput(resultOutput, args.raw, args.pretty, true, url);
}

function cmdBatchFollow(args: ParsedArgs): void {
  // Session health check — close stale tabs before starting
  runHealthCheck(args.port);

  // ★ Split URLs into cached (instant) and uncached (fetch)
  const cachedResults: string[] = [];
  const uncachedUrls: string[] = [];
  for (const url of args.positional) {
    const cached = cacheGetByUrl(url, args.offset, args.maxLen);
    if (cached) {
      cachedResults.push(cached);
    } else {
      uncachedUrls.push(url);
    }
  }

  let fetchedResults: string[] = [];
  if (uncachedUrls.length > 0) {
    const js = batchFollowCode(
      uncachedUrls,
      args.selector,
      args.timeout,
      args.port,
      args.offset,
      args.maxLen,
    );
    const { stdout, stderr, exitCode } = runBrowserHarness(js);

    if (exitCode !== 0) {
      console.log(
        JSON.stringify({
          success: false,
          error: 'batch_follow_failed',
          detail: stderr || stdout || 'unknown error',
        }),
      );
      return;
    }

    // ★ Parse and cache each result
    try {
      const parsed = JSON.parse(stdout);
      if (Array.isArray(parsed)) {
        for (const item of parsed) {
          cacheSetByUrl(
            item.url || uncachedUrls[0],
            JSON.stringify(item),
            args.offset,
            args.maxLen,
          );
        }
      }
    } catch {
      // Cache write failure is non-fatal
    }
    fetchedResults = [stdout];
  }

  // ★ Combine cached + fetched into a single JSON array
  const combined = JSON.stringify([
    ...cachedResults.map((r) => {
      try {
        return JSON.parse(r);
      } catch {
        return { content: r };
      }
    }),
    ...fetchedResults.flatMap((r) => {
      try {
        return JSON.parse(r);
      } catch {
        return [];
      }
    }),
  ]);

  // ★ Quality check on batch results (non-blocking — log only)
  if (!args.raw) {
    try {
      const parsed = JSON.parse(combined);
      if (Array.isArray(parsed)) {
        let lowQuality = 0;
        for (const item of parsed) {
          const text = item.content || '';
          const q = validateContent(text);
          if (!q.ok) lowQuality++;
        }
        if (lowQuality > 0 && lowQuality > parsed.length * 0.5) {
          // More than half are low quality — emit warning to stderr
          console.error(
            JSON.stringify({
              _quality_warning: true,
              total: parsed.length,
              low_quality: lowQuality,
              message:
                'More than 50% of results are low quality. Consider using --retry or different URLs.',
            }),
          );
        }
      }
    } catch {
      // Quality check failure is non-fatal
    }
  }

  printOutput(combined, args.raw, args.pretty);
}

function cmdSearch(args: ParsedArgs): void {
  const js = searchCode(args.positional[0], args.count, args.port);
  const { stdout, stderr, exitCode } = runBrowserHarness(js);

  if (exitCode !== 0) {
    const errMsg = stderr || stdout || 'unknown error';
    console.log(
      JSON.stringify({ success: false, error: 'search_failed', detail: errMsg }),
    );
    return;
  }

  console.log(stdout);
}

function cmdBatchSearch(args: ParsedArgs): void {
  // Session health check — close stale tabs before starting
  runHealthCheck(args.port);

  const js = batchSearchCode(args.positional, args.count, args.port);
  const { stdout, stderr, exitCode } = runBrowserHarness(js);

  if (exitCode !== 0) {
    const errMsg = stderr || stdout || 'unknown error';
    console.log(
      JSON.stringify({
        success: false,
        error: 'batch_search_failed',
        detail: errMsg,
      }),
    );
    return;
  }

  console.log(stdout);
}

function cmdBatchHarvest(args: ParsedArgs): void {
  // Session health check — close stale tabs before starting
  runHealthCheck(args.port);

  const js = batchHarvestCode(
    args.positional,
    args.count,
    args.maxPages,
    args.timeout,
    args.port,
  );
  const { stdout, stderr, exitCode } = runBrowserHarness(js);

  if (exitCode !== 0) {
    const errMsg = stderr || stdout || 'unknown error';
    console.log(
      JSON.stringify({
        success: false,
        error: 'batch_harvest_failed',
        detail: errMsg,
      }),
    );
    return;
  }

  console.log(stdout);
}

// ─── Main ────────────────────────────────────────────────────────────────

function main(): void {
  const argv = process.argv.slice(2);
  if (argv.length === 0) usage();

  const args = parseArgs(argv);

  switch (args.command) {
    case 'follow':
      cmdFollow(args);
      break;
    case 'batch-follow':
      cmdBatchFollow(args);
      break;
    case 'search':
      cmdSearch(args);
      break;
    case 'batch-search':
      cmdBatchSearch(args);
      break;
    case 'batch-harvest':
      cmdBatchHarvest(args);
      break;
    default:
      usage();
  }
}

if (import.meta.main) {
  main();
}

// ─── Re-exports for backward compatibility with tests ────────────
// These tests import from browser-automation.ts and expect these
// symbols to be available here.
export {
  connectCode, extractionCode, followCode, batchFollowCode,
  searchCode, batchSearchCode, batchHarvestCode,
  sessionHealthCheckCode, readyStatePoll, readyStateCheck
} from './templates';

export {
  qualityGateCode, validateContent
} from './quality';

export type { QualityResult } from './quality';
