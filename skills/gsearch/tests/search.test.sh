#!/usr/bin/env bash
# Test: gsearch search.sh — cmd_search function behavior
set -euo pipefail

# --- Setup ---
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
LIB="$SCRIPT_DIR/../lib"

# Source dependencies first, then search.sh
. "$LIB/common.sh"
. "$LIB/search.sh"

# --- Counters ---
pass=0
fail=0

# --- Assertion helpers ---

assert_eq() {
  local expected="$1" actual="$2" label="${3:-}"
  if [ "$actual" = "$expected" ]; then
    echo "  OK $label"
    pass=$((pass + 1))
  else
    echo "  FAIL FAIL: $label"
    echo "    expected: $expected"
    echo "    actual:   $actual"
    fail=$((fail + 1))
  fi
}

assert_contains() {
  local needle="$1" haystack="$2" label="${3:-}"
  if echo "$haystack" | grep -Fq -- "$needle"; then
    echo "  OK $label"
    pass=$((pass + 1))
  else
    echo "  FAIL FAIL: $label"
    echo "    expected to find: $needle"
    echo "    in: $haystack"
    fail=$((fail + 1))
  fi
}

assert_not_contains() {
  local needle="$1" haystack="$2" label="${3:-}"
  if echo "$haystack" | grep -Fq -- "$needle"; then
    echo "  FAIL FAIL: $label (found: $needle)"
    fail=$((fail + 1))
  else
    echo "  OK $label"
    pass=$((pass + 1))
  fi
}

assert_file_contains() {
  local needle="$1" file="$2" label="${3:-}"
  if grep -Fq -- "$needle" "$file"; then
    echo "  OK $label"
    pass=$((pass + 1))
  else
    echo "  FAIL FAIL: $label"
    echo "    expected to find in $file: $needle"
    fail=$((fail + 1))
  fi
}

# ════════════════════════════════════════════════════════════════════
# Test 1: Function definition exists
# ════════════════════════════════════════════════════════════════════

echo ""
echo "=== Test 1: Function definition ==="

if declare -F cmd_search >/dev/null 2>&1; then
  echo "  OK cmd_search function exists"
  pass=$((pass + 1))
else
  echo "  FAIL FAIL: cmd_search function not found"
  fail=$((fail + 1))
fi

# ════════════════════════════════════════════════════════════════════
# Test 2: No forbidden patterns in function body
# ════════════════════════════════════════════════════════════════════

echo ""
echo "=== Test 2: No forbidden patterns ==="

# Capture function body
FUNC_BODY=$(type cmd_search 2>/dev/null)

assert_not_contains "inject_connect" "$FUNC_BODY" "no inject_connect"
assert_not_contains "browser-harness-js" "$FUNC_BODY" "no browser-harness-js"
assert_not_contains "mktemp" "$FUNC_BODY" "no mktemp"
assert_not_contains "JSEOF" "$FUNC_BODY" "no JSEOF"
assert_not_contains "<<" "$FUNC_BODY" "no heredoc bodies"

# ════════════════════════════════════════════════════════════════════
# Test 3: Delegation to browser-automation.ts
# ════════════════════════════════════════════════════════════════════

echo ""
echo "=== Test 3: Delegation to browser-automation.ts ==="

SEARCH_FILE="$LIB/search.sh"
if [ -f "$SEARCH_FILE" ]; then
  count=$(grep -c 'browser-automation.ts' "$SEARCH_FILE" 2>/dev/null || echo 0)
  if [ "$count" -gt 0 ]; then
    echo "  OK search.sh references browser-automation.ts ($count times)"
    pass=$((pass + 1))
  else
    echo "  FAIL FAIL: search.sh does not reference browser-automation.ts"
    fail=$((fail + 1))
  fi
else
  echo "  FAIL FAIL: search.sh not found at $SEARCH_FILE"
  fail=$((fail + 1))
fi

# ════════════════════════════════════════════════════════════════════
# Test 4: Mock-based argument tests
# ════════════════════════════════════════════════════════════════════

echo ""
echo "=== Test 4: Mock-based argument tests ==="

# --- Setup mock environment ---

# Create temp file for recording bun args
BUN_ARGS_FILE="$(mktemp /tmp/gsearch-test-bun-args-XXXXXX)"
BUN_MOCK_OUTPUT='[]'
BUN_CALL_COUNT=0

# For safety, use a temp dir that definitely exists
MOCK_CDP_SCRIPTS="/tmp/gsearch-test-cdp-scripts"
mkdir -p "$MOCK_CDP_SCRIPTS"

# Create stub browser-automation.ts for _cdp_call's file-existence check
# _cdp_call checks for ${CDP_SCRIPTS}/browser-automation.ts before calling bun
cat > "$MOCK_CDP_SCRIPTS/browser-automation.ts" << 'STUB'
#!/usr/bin/env bun
// Mock for tests - always returns empty array as JSON
console.log('[]');
STUB
chmod +x "$MOCK_CDP_SCRIPTS/browser-automation.ts"

# Mock bun() to record args
bun() {
  BUN_CALL_COUNT=$((BUN_CALL_COUNT + 1))
  printf '%s\n' "$*" > "$BUN_ARGS_FILE"
  echo "$BUN_MOCK_OUTPUT"
}

# Override globals used by cmd_search
CDP_SCRIPTS="$MOCK_CDP_SCRIPTS"
GSEARCH_CDP_PORT="9999"

reset_mock() {
  BUN_CALL_COUNT=0
  : > "$BUN_ARGS_FILE"
}

cleanup() {
  rm -f "$BUN_ARGS_FILE"
  rm -rf "$MOCK_CDP_SCRIPTS"
}
trap cleanup EXIT

# --- Test 4a: basic query passthrough ---
echo "  --- 4a: basic query passthrough ---"
reset_mock

cmd_search --count 3 "test query" 2>/dev/null || true

ARGS=$(cat "$BUN_ARGS_FILE")
echo "    bun args: $ARGS"
assert_contains "test query" "$ARGS" "4a: query 'test query' passed to bun"
assert_contains "--count 3" "$ARGS" "4a: --count 3 passed to bun"
assert_contains "--port 9999" "$ARGS" "4a: --port 9999 passed to bun"
assert_contains "browser-automation.ts" "$ARGS" "4a: browser-automation.ts in args"

# --- Test 4b: --pretty flag ---
echo "  --- 4b: --pretty flag ---"
reset_mock

cmd_search --pretty "query2" 2>/dev/null || true

ARGS=$(cat "$BUN_ARGS_FILE")
echo "    bun args: $ARGS"
assert_contains "query2" "$ARGS" "4b: query 'query2' passed to bun"
assert_not_contains "--pretty" "$ARGS" "4b: --pretty NOT passed to bun (consumed as flag)"

# --- Test 4c: default count (10) ---
echo "  --- 4c: default count ---"
reset_mock

cmd_search "default count query" 2>/dev/null || true

ARGS=$(cat "$BUN_ARGS_FILE")
echo "    bun args: $ARGS"
assert_contains "default count query" "$ARGS" "4c: query text passed"
# Default count is 10 per cmd_search
assert_contains "--count 10" "$ARGS" "4c: default --count 10"

# --- Test 4d: empty args (no query) triggers die_usage ---
echo "  --- 4d: no-argument calls die_usage ---"
reset_mock

# Capture stderr and ensure exit 1
test_die_on_no_args() {
  ( cmd_search 2>/dev/null; ) && return 1 || return 0
}
if test_die_on_no_args; then
  echo "  OK 4d: cmd_search with no args exits 1 (die_usage)"
  pass=$((pass + 1))
else
  echo "  FAIL FAIL: 4d: cmd_search with no args did not exit 1"
  fail=$((fail + 1))
fi

# --- Test 4e: --count with non-numeric triggers die_usage ---
echo "  --- 4e: --count with non-numeric ---"
reset_mock

test_die_on_bad_count() {
  ( cmd_search --count abc "query" 2>/dev/null; ) && return 1 || return 0
}
if test_die_on_bad_count; then
  echo "  OK 4e: --count abc exits 1 (die_usage)"
  pass=$((pass + 1))
else
  echo "  FAIL FAIL: 4e: --count abc did not exit 1"
  fail=$((fail + 1))
fi

# --- Test 4f: unknown option triggers die_usage ---
echo "  --- 4f: unknown option ---"
reset_mock

test_die_on_unknown_opt() {
  ( cmd_search --bogus "query" 2>/dev/null; ) && return 1 || return 0
}
if test_die_on_unknown_opt; then
  echo "  OK 4f: --bogus exits 1 (die_usage)"
  pass=$((pass + 1))
else
  echo "  FAIL FAIL: 4f: --bogus did not exit 1"
  fail=$((fail + 1))
fi

# ════════════════════════════════════════════════════════════════════
# === NEW TESTS === _validate_search_results (7 tests)
# ════════════════════════════════════════════════════════════════════

echo ""
echo "=== _validate_search_results ==="

# Test 5a: valid JSON array with title+url returns 0
validate_test_0() {
  ( _validate_search_results '[{"title":"Test","url":"https://example.com"}]' 2>/dev/null; ) && return 0 || return 1
}
if validate_test_0; then
  echo "  OK 5a: valid result returns 0"
  pass=$((pass + 1))
else
  echo "  FAIL FAIL: 5a: valid result should return 0"
  fail=$((fail + 1))
fi

# Test 5b: empty JSON array returns 1
validate_test_1() {
  ( _validate_search_results '[]' 2>/dev/null; ) && return 1 || return 0
}
if validate_test_1; then
  echo "  OK 5b: empty array returns 1"
  pass=$((pass + 1))
else
  echo "  FAIL FAIL: 5b: empty array should return 1"
  fail=$((fail + 1))
fi

# Test 5c: empty string returns 1
validate_test_2() {
  ( _validate_search_results '' 2>/dev/null; ) && return 1 || return 0
}
if validate_test_2; then
  echo "  OK 5c: empty string returns 1"
  pass=$((pass + 1))
else
  echo "  FAIL FAIL: 5c: empty string should return 1"
  fail=$((fail + 1))
fi

# Test 5d: malformed JSON returns 1
validate_test_3() {
  ( _validate_search_results 'not json' 2>/dev/null; ) && return 1 || return 0
}
if validate_test_3; then
  echo "  OK 5d: malformed JSON returns 1"
  pass=$((pass + 1))
else
  echo "  FAIL FAIL: 5d: malformed JSON should return 1"
  fail=$((fail + 1))
fi

# Test 5e: object missing "title" field returns 1
validate_test_4() {
  ( _validate_search_results '[{"url":"https://example.com"}]' 2>/dev/null; ) && return 1 || return 0
}
if validate_test_4; then
  echo "  OK 5e: missing title returns 1"
  pass=$((pass + 1))
else
  echo "  FAIL FAIL: 5e: missing title should return 1"
  fail=$((fail + 1))
fi

# Test 5f: object missing "url" field returns 1
validate_test_5() {
  ( _validate_search_results '[{"title":"No URL"}]' 2>/dev/null; ) && return 1 || return 0
}
if validate_test_5; then
  echo "  OK 5f: missing url returns 1"
  pass=$((pass + 1))
else
  echo "  FAIL FAIL: 5f: missing url should return 1"
  fail=$((fail + 1))
fi

# Test 5g: some valid among invalid — at least one valid entry returns 0
validate_test_6() {
  ( _validate_search_results '[{"title":"","url":"https://x.com"},{"title":"Real","url":"https://real.com"}]' 2>/dev/null; ) && return 0 || return 1
}
if validate_test_6; then
  echo "  OK 5g: at least one valid entry returns 0"
  pass=$((pass + 1))
else
  echo "  FAIL FAIL: 5g: at least one valid entry should return 0"
  fail=$((fail + 1))
fi

# ════════════════════════════════════════════════════════════════════
# Summary
# ════════════════════════════════════════════════════════════════════

echo ""
echo "════════════════════════════════════════════"
echo "  Results: $pass passed, $fail failed"
echo "════════════════════════════════════════════"

if [ "$fail" -eq 0 ]; then
  exit 0
else
  exit 1
fi
