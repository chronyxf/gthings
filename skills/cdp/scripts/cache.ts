#!/usr/bin/env bun
/**
 * cache.ts — SHA-256 disk-based content cache
 *
 * Pure stateless service: all functions take inputs and return results.
 * No global state, no side effects beyond explicit file I/O on cache
 * get/set operations.
 *
 * Cache location: /tmp/nyx-search-cache/<sha256>.json
 * TTL: 1 hour (files older than this are re-fetched)
 *
 * DESIGN:
 *   - Deterministic keys via SHA-256 of (url | offset | max)
 *   - Cache misses return null (not throw)
 *   - Expired files are removed on read (lazy eviction)
 *   - Write failures are silent — cache is an optimization, not a guarantee
 */

import { createHash } from 'node:crypto';
import {
  existsSync,
  mkdirSync,
  readFileSync,
  statSync,
  unlinkSync,
  writeFileSync,
} from 'node:fs';

export const CACHE_DIR = '/tmp/nyx-search-cache';
export const CACHE_TTL_MS = 60 * 60 * 1000; // 1 hour

/**
 * cacheKey: Pure — returns SHA-256 hex digest for cache lookup.
 * Combines (url, offset, max) into a deterministic key.
 */
export function cacheKey(url: string, offset = 0, max = 15000): string {
  return createHash('sha256')
    .update(`${url}|${offset}|${max}`)
    .digest('hex');
}

/**
 * cacheGet: Retrieve cached data by key.
 * Returns null on miss or expiry.
 * Checks file mtime against 1-hour TTL. Expired files are removed.
 */
export function cacheGet(key: string): string | null {
  try {
    const path = `${CACHE_DIR}/${key}.json`;
    if (!existsSync(path)) return null;
    const stat = statSync(path);
    if (Date.now() - stat.mtimeMs > CACHE_TTL_MS) {
      unlinkSync(path); // lazy eviction
      return null;
    }
    return readFileSync(path, 'utf8');
  } catch {
    return null; // cache miss on any error
  }
}

/**
 * cacheSet: Write data to cache by key.
 * Creates cache directory if needed.
 * Cache write failures are non-fatal (silently ignored).
 */
export function cacheSet(key: string, data: string): void {
  try {
    if (!existsSync(CACHE_DIR)) mkdirSync(CACHE_DIR, { recursive: true });
    writeFileSync(`${CACHE_DIR}/${key}.json`, data, 'utf8');
  } catch {
    // Cache write failure is non-fatal
  }
}

/**
 * cacheGetByUrl: Convenience — cacheKey + cacheGet combined.
 * Returns cached JSON string or null.
 */
export function cacheGetByUrl(
  url: string,
  offset?: number,
  max?: number,
): string | null {
  return cacheGet(cacheKey(url, offset, max));
}

/**
 * cacheSetByUrl: Convenience — cacheKey + cacheSet combined.
 * Stores data under key derived from (url, offset, max).
 */
export function cacheSetByUrl(
  url: string,
  data: string,
  offset?: number,
  max?: number,
): void {
  cacheSet(cacheKey(url, offset, max), data);
}
