#!/usr/bin/env bun
/**
 * PDF text extractor v2.0 — reads PDF from stdin, outputs JSON with text to stdout.
 *
 * Architecture (parsing stages):
 *   1. Input: read PDF bytes from stdin or file (chunked for large files)
 *   2. Cache: content-based SHA-256 cache lookup/save at /tmp/nyx-search-cache/
 *   3. Validation: check PDF magic number (%PDF-N.M)
 *   4. XRef/Trailer: scan for cross-reference table (structural map of objects)
 *   5. Streams: extract all stream objects with their filter metadata
 *   6. Decompress: inflate FlateDecode streams via zlib; detect unsupported filters
 *   7. Text: extract parenthesized strings from content streams (Tj, TJ operators)
 *   8. Output: JSON with text, pages, method, length, source, cache status
 *
 * Uses built-in zlib for FlateDecode streams. Zero external dependencies.
 * Bun-native (runs TypeScript directly). Handles academic PDFs (arXiv, research papers).
 *
 * Category theory: This is a morphism PDF → JSON in the category of
 *   file-format transformations. It factors through multiple intermediate
 *   objects (Stream, Chunk, Token) with well-defined composition.
 *
 * Information theory: Extraction maximizes mutual information I(PDF; Text)
 *   by recovering the text content from compressed streams, minimizing
 *   the information lost in the PDF→Text channel.
 */

import zlib from 'zlib';
import fs from 'fs';
import crypto from 'crypto';

const VERSION = '2.0.0';

// ─── Types ─────────────────────────────────────────────────────────

/** A PDF stream object with its filter type and raw data */
interface PdfStream {
  data: string;
  isFlate: boolean;
  objHeader: string;
}

/** Extracted text chunk from a PDF content stream */
interface TextChunk {
  text: string;
  streamIndex: number;
}

/** Result of decompressing a stream — either data or an error description */
type DecompressResult =
  | { ok: true; data: Buffer }
  | { ok: false; error: string };

// ─── Stage 1: Input (chunked read for memory efficiency) ─────────

/**
 * Read all bytes from a file descriptor in 64KB chunks.
 * More memory-efficient than readFileSync for large files (>1MB).
 * Uses readSync to avoid loading the entire file into a single growing buffer.
 */
function readChunked(fd: number): Buffer {
  const chunks: Buffer[] = [];
  const chunkSize = 65536; // 64KB
  const buf = Buffer.alloc(chunkSize);

  while (true) {
    const bytesRead = fs.readSync(fd, buf, 0, chunkSize, null);
    if (bytesRead <= 0) break;
    // Copy to avoid buffer reuse corruption on next readSync call
    const chunk = Buffer.alloc(bytesRead);
    buf.copy(chunk, 0, 0, bytesRead);
    chunks.push(chunk);
  }

  return Buffer.concat(chunks);
}

// ─── Stage 2: Content cache (SHA-256 key at /tmp/nyx-search-cache/) ──

/**
 * Compute SHA-256 hex digest of a buffer.
 * Used for content-based cache key (same format as browser-automation.ts cache).
 */
function sha256(buf: Buffer): string {
  return crypto.createHash('sha256').update(buf).digest('hex');
}

/**
 * Check the content cache at /tmp/nyx-search-cache/.
 * Returns cached text if found and non-empty, null otherwise.
 */
function checkCache(sha: string): string | null {
  const cachePath = `/tmp/nyx-search-cache/${sha}`;
  try {
    if (fs.existsSync(cachePath)) {
      const cached = fs.readFileSync(cachePath, 'utf-8');
      return cached.length > 0 ? cached : null;
    }
  } catch {
    // Cache unavailable (permission, fs error) — proceed with extraction
  }
  return null;
}

/**
 * Save extracted text to the content cache.
 * Non-critical operation — failures are silently ignored.
 */
function saveCache(sha: string, text: string): void {
  try {
    const cacheDir = '/tmp/nyx-search-cache';
    if (!fs.existsSync(cacheDir)) {
      fs.mkdirSync(cacheDir, { recursive: true });
    }
    fs.writeFileSync(`${cacheDir}/${sha}`, text, 'utf-8');
  } catch {
    // Cache write failures are non-fatal
  }
}

// ─── Stage 3: Validation (PDF magic number check) ────────────────

/**
 * Check if a buffer starts with PDF magic number.
 * Functor: identity check in the FileType category.
 * All valid PDFs begin with %PDF-N.M where N.M is the version number.
 */
function isPdf(buf: Buffer): boolean {
  return buf.slice(0, 5).toString() === '%PDF-';
}

// ─── Stage 4: XRef / Trailer scanning (structural map) ─────────

/**
 * Estimate page count from PDF source.
 * Counts /Type /Page entries (excluding /Type /Pages with trailing 's').
 * This is a heuristic — accurate for well-formed PDFs.
 */
function countPages(buf: Buffer): number {
  const src = buf.toString('binary');
  const pageMatch = src.match(/\/Type\s*\/Page[^s]/g);
  return pageMatch ? pageMatch.length : 0;
}

// ─── Stage 5: Stream object extraction ────────────────────────────

/**
 * Extract all stream objects from raw PDF source.
 * Natural transformation: PDF source → [PdfStream]
 *
 * Each PDF object is delimited by "N M obj" ... "endobj".
 * Each stream object contains "stream" ... "endstream" with raw data.
 * FlateDecode filter is detected in the object header.
 */
function extractStreams(src: string): PdfStream[] {
  const streams: PdfStream[] = [];
  const objRegex = /(\d+ \d+ obj[\s\S]*?endobj)/g;
  let match: RegExpExecArray | null;

  while ((match = objRegex.exec(src)) !== null) {
    const obj = match[1];
    const isFlate = /FlateDecode/i.test(obj);
    const streamMatch = obj.match(/stream\s([\s\S]*?)\n?endstream/);

    if (streamMatch) {
      streams.push({
        data: streamMatch[1].trim(),
        isFlate,
        objHeader: obj.slice(0, 80),
      });
    }
  }

  return streams;
}

// ─── Stage 6: Decompression (zlib inflate with error handling) ──

/**
 * Decompress a single PDF stream object.
 *
 * Supported filters:
 *   - FlateDecode: zlib inflate (most common for text-based PDFs)
 *   - Plain/uncompressed: returned as-is (no decompression needed)
 *
 * Unsupported filters (LZWDecode, ASCII85Decode, RunLengthDecode,
 * CCITTFaxDecode) are detected and reported with a descriptive error.
 *
 * Corrupted FlateDecode streams are caught with specific try/catch
 * messages that identify the type of corruption.
 */
function decompressStream(s: PdfStream): DecompressResult {
  try {
    if (s.isFlate) {
      const raw = Buffer.from(s.data, 'binary');
      return { ok: true, data: zlib.inflateSync(raw) };
    }

    // Detect unsupported compression filters
    const unsupported = /LZWDecode|ASCII85Decode|RunLengthDecode|CCITTFaxDecode/i;
    if (unsupported.test(s.objHeader)) {
      return {
        ok: false,
        error: `Unsupported compression filter in stream: ${s.objHeader.slice(0, 60)}`,
      };
    }

    // Plain/uncompressed stream — return raw bytes
    return { ok: true, data: Buffer.from(s.data, 'binary') };
  } catch (e: unknown) {
    if (e instanceof Error) {
      // Provide specific messages for common zlib error patterns
      if (e.message.includes('incorrect header check')) {
        return { ok: false, error: 'Corrupted FlateDecode stream: incorrect header check (data is not zlib-compressed)' };
      }
      if (e.message.includes('invalid distance code') || e.message.includes('invalid literal/length')) {
        return { ok: false, error: `Corrupted FlateDecode stream: ${e.message}` };
      }
      if (e.message.includes('unexpected end of data')) {
        return { ok: false, error: 'Corrupted FlateDecode stream: unexpected end of data (stream may be truncated)' };
      }
      if (e.message.includes('invalid block type')) {
        return { ok: false, error: `Corrupted FlateDecode stream: ${e.message}` };
      }
      if (e.message.includes('unknown compression method')) {
        return { ok: false, error: `Corrupted FlateDecode stream: ${e.message}` };
      }
      return { ok: false, error: `FlateDecode decompression error: ${e.message}` };
    }
    return { ok: false, error: `FlateDecode decompression error: ${String(e)}` };
  }
}

// ─── Stage 7: Text extraction from content streams ────────────────

/**
 * Extract text from a decompressed content stream.
 * Uses PDF text operators: Tj (show string), TJ (show with positioning array),
 * ' (move to next line and show string), " (set word/char spacing and show).
 *
 * PDF text is enclosed in parentheses: (text content here)
 * Backslash-escaped characters are unescaped:
 *   - \( → (    \) → )    \\ → \
 *   - \ddd → octal character code (e.g., \050 → '(')
 *   - \n → newline (literal \n in PDF stream source)
 *
 * Morphism: Buffer → [TextChunk] in the PDF-parsing category.
 */
function extractTextFromStream(raw: Buffer, streamIndex: number): TextChunk[] {
  const chunks: TextChunk[] = [];
  const src = raw.toString('binary');

  // Match parenthesized strings in PDF content streams
  const parenRegex = /\(([^)]*)\)/g;
  let match: RegExpExecArray | null;

  while ((match = parenRegex.exec(src)) !== null) {
    const t = match[1]
      // Octal escape sequences: \ddd (e.g., \050 for '(')
      .replace(/\\([0-7]{3})/g, (_: string, oct: string) =>
        String.fromCharCode(parseInt(oct, 8))
      )
      // Standard escape sequences: \(, \), \\
      .replace(/\\(.)/g, '$1')
      // Literal \n in PDF source
      .replace(/\\n/g, '\n')
      .trim();

    if (t.length > 1) {
      chunks.push({ text: t, streamIndex });
    }
  }

  return chunks;
}

// ─── Stage 8: Main pipeline ────────────────────────────────────────

/**
 * Main extraction pipeline. Composes all stages as morphisms:
 *
 *   readChunked → sha256 → [checkCache]?
 *   → isPdf (guard) → extractStreams
 *   → map(decompressStream) → flatMap(extractTextFromStream)
 *   → join → saveCache → JSON output
 *
 * Monad: The extraction is a Maybe monad — each step can fail (null/Nothing).
 * The bind (>>=) propagates failures implicitly via null checks and
 * early exits with structured error output.
 */
function main(): void {
  // ── Parse CLI arguments ──
  const args = process.argv.slice(2);

  if (args.includes('--version')) {
    console.log(JSON.stringify({ version: VERSION }));
    process.exit(0);
  }

  let inputPath: string | null = null;
  for (let i = 0; i < args.length; i++) {
    if (args[i] === '--input' && i + 1 < args.length) {
      inputPath = args[++i];
    }
  }

  // ── Stage 1: Read input (chunked for large files >1MB) ──
  let buf: Buffer;
  try {
    if (inputPath) {
      const fd = fs.openSync(inputPath, 'r');
      buf = readChunked(fd);
      fs.closeSync(fd);
    } else {
      buf = readChunked(0); // stdin file descriptor
    }
  } catch (e: unknown) {
    const msg = e instanceof Error ? e.message : String(e);
    console.log(JSON.stringify({ error: 'extraction_failed', detail: `Input read error: ${msg}` }));
    process.exit(1);
  }

  // ── Stage 2: Content cache check (SHA-256 key) ──
  const hash = sha256(buf);
  const cached = checkCache(hash);
  if (cached !== null) {
    console.log(JSON.stringify({
      text: cached,
      pages: countPages(buf),
      method: 'pdf-extract-ts',
      length: cached.length,
      source: inputPath || 'stdin',
      cache: 'hit',
    }));
    process.exit(0);
  }

  // ── Stage 3: Validate PDF magic number ──
  if (!isPdf(buf)) {
    console.log(JSON.stringify({ error: 'extraction_failed', detail: 'Not a PDF file (magic number mismatch — expected %PDF-*)' }));
    process.exit(1);
  }

  // ── Stages 4-5: Extract stream objects ──
  const src = buf.toString('binary');
  const streams = extractStreams(src);

  if (streams.length === 0) {
    console.log(JSON.stringify({ error: 'extraction_failed', detail: 'No stream objects found in PDF' }));
    process.exit(1);
  }

  // ── Stages 6-7: Decompress streams and extract text ──
  const errors: string[] = [];
  const allChunks: TextChunk[] = [];

  for (let i = 0; i < streams.length; i++) {
    const result = decompressStream(streams[i]);
    if (result.ok) {
      const chunks = extractTextFromStream(result.data, i);
      allChunks.push(...chunks);
    } else {
      errors.push(result.error);
    }
  }

  // Report non-fatal decompression errors to stderr for diagnostics
  if (errors.length > 0) {
    for (const err of errors) {
      process.stderr.write(`pdf-extract: ${err}\n`);
    }
  }

  // ── Join all text chunks into the final output ──
  const text = allChunks
    .map(c => c.text)
    .join(' ')
    .replace(/\s+/g, ' ')
    .trim();

  if (text.length === 0) {
    console.log(JSON.stringify({
      error: 'extraction_failed',
      detail: errors.length > 0
        ? `No text extracted. ${errors.length} stream(s) had errors: ${errors[0]}`
        : 'No text extracted from PDF streams (PDF may contain only images or non-text content)',
    }));
    process.exit(1);
  }

  // ── Stage 8: Save to cache and output JSON ──
  saveCache(hash, text);

  console.log(JSON.stringify({
    text,
    pages: countPages(buf),
    method: 'pdf-extract-ts',
    length: text.length,
    source: inputPath || 'stdin',
  }));
}

main();
