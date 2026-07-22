#!/usr/bin/env bash
# Test: gsearch batch operations — URL handling in mock bun()
set -euo pipefail

# --- Setup ---
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
LIB="$SCRIPT_DIR/../lib"
CDP_SCRIPTS="$SCRIPT_DIR/../../cdp/scripts"

# Source the batch lib
. "$LIB/common.sh" 2>/dev/null || true
. "$LIB/batch.sh"

# --- Mock globals ---
BUN_ARGS_FILE="$(mktemp /tmp/gsearch-test-bun-args-XXXXXX)"
BUN_CALL_COUNT=0
BUN_MOCK_OUTPUT='{"success":true,"results":[]}'

# --- Mock bun() ---
# Uses printf instead of echo to avoid backslash interpretation on ://
bun() {
  BUN_CALL_COUNT=$((BUN_CALL_COUNT + 1))
  printf '%s\n' "$*" > "$BUN_ARGS_FILE"
  echo "$BUN_MOCK_OUTPUT"
}

# --- Assertion helpers ---

assert_bun_contains() {
  local expected="$1"
  if grep -F -- "$expected" "$BUN_ARGS_FILE" >/dev/null 2>&1; then
    echo "  OK args contain: $expected"
  else
    echo "  FAIL FAIL: args missing: $expected"
    echo "    args file contents: $(cat "$BUN_ARGS_FILE")"
    exit 1
  fi
}

assert_bun_args_equal() {
  local expected="$1"
  local actual
  actual="$(cat "$BUN_ARGS_FILE")"
  if [ "$actual" = "$expected" ]; then
    echo "  OK args exactly: $expected"
  else
    echo "  FAIL FAIL: args mismatch"
    echo "    expected: $expected"
    echo "    actual:   $actual"
    exit 1
  fi
}

reset_mock() {
  BUN_CALL_COUNT=0
  : > "$BUN_ARGS_FILE"
}

cleanup() {
  rm -f "$BUN_ARGS_FILE"
}
trap cleanup EXIT

# --- Test: URL with :// is preserved in bun() args ---
echo ""
echo "=== Test 1: Mock bun() preserves URL with :// ==="
reset_mock

# Simulate what batch-follow does: pass a URL with ://
bun "${CDP_SCRIPTS}/browser-automation.ts" batch-follow "https://arxiv.org/abs/2305.12345"

echo "  args file contents: $(cat "$BUN_ARGS_FILE")"
# The URL should be intact — no backslash corruption
if grep -Fq "https://arxiv.org/abs/2305.12345" "$BUN_ARGS_FILE"; then
  echo "  OK URL preserved correctly"
else
  echo "  FAIL FAIL: URL corrupted by echo"
  echo "    expected to find: https://arxiv.org/abs/2305.12345"
  echo "    got: $(cat "$BUN_ARGS_FILE")"
  exit 1
fi

# --- Test 2: batch-follow command line construction ---
echo ""
echo "=== Test 2: batch-follow constructs correct bun command ==="
reset_mock

# We need to call the actual cmd_batch_follow from batch.sh
# It requires bun to be available. Since bun is mocked above, this should work.
# But cmd_batch_follow passes ${urls[@]} which would expand, and our mock captures it.

# Call cmd_batch_follow with a URL
cmd_batch_follow "https://arxiv.org/abs/2305.12345" 2>/dev/null || true

echo "  args file contents: $(cat "$BUN_ARGS_FILE")"
if grep -Fq "batch-follow" "$BUN_ARGS_FILE"; then
  echo "  OK batch-follow subcommand present"
else
  echo "  FAIL FAIL: batch-follow subcommand missing"
  exit 1
fi

# The URL should be intact (no backslash corruption from echo)
if grep -Fq "https://arxiv.org/abs/2305.12345" "$BUN_ARGS_FILE"; then
  echo "  OK URL preserved in batch-follow call"
else
  echo "  FAIL FAIL: URL corrupted in batch-follow call"
  echo "    got: $(cat "$BUN_ARGS_FILE")"
  exit 1
fi

# --- Test 3: batch-search command line construction ---
echo ""
echo "=== Test 3: batch-search constructs correct bun command ==="
reset_mock

cmd_batch_search --count 3 "machine learning" "transformer models" 2>/dev/null || true

echo "  args file contents: $(cat "$BUN_ARGS_FILE")"
if grep -Fq "batch-search" "$BUN_ARGS_FILE"; then
  echo "  OK batch-search subcommand present"
else
  echo "  FAIL FAIL: batch-search subcommand missing"
  exit 1
fi

# --- Test 4: batch-harvest command line construction ---
echo ""
echo "=== Test 4: batch-harvest constructs correct bun command ==="
reset_mock

cmd_batch_harvest --count 2 --max 3 "AI research" "quantum computing" 2>/dev/null || true

echo "  args file contents: $(cat "$BUN_ARGS_FILE")"
if grep -Fq "batch-harvest" "$BUN_ARGS_FILE"; then
  echo "  OK batch-harvest subcommand present"
else
  echo "  FAIL FAIL: batch-harvest subcommand missing"
  exit 1
fi

# --- Test 5: Mock with multiple URLs containing :// ---
echo ""
echo "=== Test 5: Multiple URLs with :// preserved ==="
reset_mock

bun "${CDP_SCRIPTS}/browser-automation.ts" batch-follow \
  "https://arxiv.org/abs/2305.12345" \
  "https://example.com/page?q=hello&x=1" \
  "http://localhost:8080/test"

ARGS="$(cat "$BUN_ARGS_FILE")"
echo "  args file contents: $ARGS"

if echo "$ARGS" | grep -Fq "https://arxiv.org/abs/2305.12345"; then
  echo "  OK URL 1 preserved"
else
  echo "  FAIL FAIL: URL 1 corrupted"
  exit 1
fi

if echo "$ARGS" | grep -Fq "https://example.com/page?q=hello&x=1"; then
  echo "  OK URL 2 preserved"
else
  echo "  FAIL FAIL: URL 2 corrupted"
  exit 1
fi

if echo "$ARGS" | grep -Fq "http://localhost:8080/test"; then
  echo "  OK URL 3 preserved"
else
  echo "  FAIL FAIL: URL 3 corrupted"
  exit 1
fi

# ════════════════════════════════════════════════════════════════════
# === NEW TESTS === _rewrite_arxiv_url (6 tests)
# ════════════════════════════════════════════════════════════════════

echo ""
echo "=== _rewrite_arxiv_url ==="

# Test 6: PDF URL converted to abstract URL
RESULT=$(_rewrite_arxiv_url "https://arxiv.org/pdf/2401.12345")
echo "  input:    https://arxiv.org/pdf/2401.12345"
echo "  output:   $RESULT"
if [ "$RESULT" = "https://arxiv.org/abs/2401.12345" ]; then
  echo "  OK _rewrite_arxiv_url pdf → abs"
else
  echo "  FAIL FAIL: expected https://arxiv.org/abs/2401.12345, got $RESULT"
  exit 1
fi

# Test 7: Non-arxiv URL unchanged
RESULT=$(_rewrite_arxiv_url "https://example.com/paper.pdf")
echo "  input:    https://example.com/paper.pdf"
echo "  output:   $RESULT"
if [ "$RESULT" = "https://example.com/paper.pdf" ]; then
  echo "  OK _rewrite_arxiv_url non-arxiv unchanged"
else
  echo "  FAIL FAIL: expected https://example.com/paper.pdf, got $RESULT"
  exit 1
fi

# Test 8: arXiv PDF with .pdf suffix stripped
RESULT=$(_rewrite_arxiv_url "https://arxiv.org/pdf/2401.12345.pdf")
echo "  input:    https://arxiv.org/pdf/2401.12345.pdf"
echo "  output:   $RESULT"
if [ "$RESULT" = "https://arxiv.org/abs/2401.12345" ]; then
  echo "  OK _rewrite_arxiv_url pdf with .pdf suffix stripped"
else
  echo "  FAIL FAIL: expected https://arxiv.org/abs/2401.12345, got $RESULT"
  exit 1
fi

# Test 9: arXiv abs URL already — stays unchanged
RESULT=$(_rewrite_arxiv_url "https://arxiv.org/abs/2401.12345")
echo "  input:    https://arxiv.org/abs/2401.12345"
echo "  output:   $RESULT"
if [ "$RESULT" = "https://arxiv.org/abs/2401.12345" ]; then
  echo "  OK _rewrite_arxiv_url abs unchanged"
else
  echo "  FAIL FAIL: expected https://arxiv.org/abs/2401.12345, got $RESULT"
  exit 1
fi

# Test 10: export.arxiv.org rewritten to arxiv.org
RESULT=$(_rewrite_arxiv_url "https://export.arxiv.org/pdf/2401.12345")
echo "  input:    https://export.arxiv.org/pdf/2401.12345"
echo "  output:   $RESULT"
if [ "$RESULT" = "https://arxiv.org/abs/2401.12345" ]; then
  echo "  OK _rewrite_arxiv_url export.arxiv.org → arxiv.org abs"
else
  echo "  FAIL FAIL: expected https://arxiv.org/abs/2401.12345, got $RESULT"
  exit 1
fi

# Test 11: abs URL with .pdf suffix stripped
RESULT=$(_rewrite_arxiv_url "https://arxiv.org/abs/2401.12345.pdf")
echo "  input:    https://arxiv.org/abs/2401.12345.pdf"
echo "  output:   $RESULT"
if [ "$RESULT" = "https://arxiv.org/abs/2401.12345" ]; then
  echo "  OK _rewrite_arxiv_url abs .pdf suffix stripped"
else
  echo "  FAIL FAIL: expected https://arxiv.org/abs/2401.12345, got $RESULT"
  exit 1
fi

# --- All tests passed ---
echo ""
echo "════════════════════════════════════════════"
echo "  All tests passed! (exit 0)"
echo "════════════════════════════════════════════"
exit 0
