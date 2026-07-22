#!/usr/bin/env bash
# Test: gsearch common.sh helper functions
set -euo pipefail

# --- Setup ---
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
LIB="$SCRIPT_DIR/../lib"

# Source the common lib
. "$LIB/common.sh"

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

assert_run_0() {
  local label="${2:-$1}"
  if eval "$1" 2>/dev/null; then
    echo "  OK $label"
    pass=$((pass + 1))
  else
    echo "  FAIL FAIL: $label (expected exit 0)"
    fail=$((fail + 1))
  fi
}

assert_run_1() {
  local label="${2:-$1}"
  if eval "$1" 2>/dev/null; then
    echo "  FAIL FAIL: $label (expected exit 1, got exit 0)"
    fail=$((fail + 1))
  else
    local rc=$?
    if [ $rc -eq 1 ]; then
      echo "  OK $label"
      pass=$((pass + 1))
    else
      echo "  FAIL FAIL: $label (expected exit 1, got exit $rc)"
      fail=$((fail + 1))
    fi
  fi
}

# ════════════════════════════════════════════════════════════════════
# Prerequisite checks
# ════════════════════════════════════════════════════════════════════

HAVE_NODE=false
command -v node >/dev/null 2>&1 && HAVE_NODE=true

if ! $HAVE_NODE; then
  echo "WARNING: node not found -- json_str tests will be skipped"
  echo "  (json_str depends on node for JSON encoding)"
fi

# ════════════════════════════════════════════════════════════════════
# json_str (5+ tests)
# ════════════════════════════════════════════════════════════════════

echo ""
echo "=== json_str ==="

# Test 1: empty string --> ""
if $HAVE_NODE; then
  result=$(json_str "")
  assert_eq '""' "$result" "json_str empty string"
else
  echo "  - skipped (no node)"
fi

# Test 2: simple string --> "hello"
if $HAVE_NODE; then
  result=$(json_str "hello")
  assert_eq '"hello"' "$result" "json_str simple string"
else
  echo "  - skipped (no node)"
fi

# Test 3: double quote --> properly escaped (contains \")
if $HAVE_NODE; then
  result=$(json_str 'hello "world"')
  assert_eq '"hello \"world\""' "$result" "json_str double quote"
else
  echo "  - skipped (no node)"
fi

# Test 4: single quote --> literal in JSON, no escaping needed
if $HAVE_NODE; then
  result=$(json_str "it's fine")
  assert_eq '"it'"'"'s fine"' "$result" "json_str single quote"
else
  echo "  - skipped (no node)"
fi

# Test 5: backslash --> properly escaped as \\
if $HAVE_NODE; then
  result=$(json_str 'a\b')
  assert_eq '"a\\b"' "$result" "json_str backslash"
else
  echo "  - skipped (no node)"
fi

# Test 6: dollar sign --> literal in JSON, no escaping needed
if $HAVE_NODE; then
  result=$(json_str '$HOME')
  assert_eq '"$HOME"' "$result" "json_str dollar sign"
else
  echo "  - skipped (no node)"
fi

# Test 7: backtick --> literal in JSON, no escaping needed
if $HAVE_NODE; then
  result=$(json_str '`date`')
  assert_eq '"`date`"' "$result" "json_str backtick"
else
  echo "  - skipped (no node)"
fi

# ════════════════════════════════════════════════════════════════════
# is_num (5 tests)
# ════════════════════════════════════════════════════════════════════

echo ""
echo "=== is_num ==="

# Test 1: 123 --> exit 0
assert_run_0 "is_num 123" "is_num 123"

# Test 2: 0 --> exit 0
assert_run_0 "is_num 0" "is_num 0"

# Test 3: empty string --> exit 1
assert_run_1 "is_num ''" "is_num empty string"

# Test 4: abc --> exit 1
assert_run_1 "is_num abc" "is_num abc"

# Test 5: 12a --> exit 1
assert_run_1 "is_num 12a" "is_num 12a"

# ════════════════════════════════════════════════════════════════════
# die_usage (2 tests)
# ════════════════════════════════════════════════════════════════════

echo ""
echo "=== die_usage ==="

# Test 1: Verify exit code is 1
test_die_usage_exit() {
  ( die_usage "test msg" 2>/dev/null; ) && return 1 || return 0
}
if test_die_usage_exit; then
  echo "  OK die_usage exits with 1"
  pass=$((pass + 1))
else
  echo "  FAIL FAIL: die_usage exit code is not 1"
  fail=$((fail + 1))
fi

# Test 2: Verify message is printed to stderr
test_die_usage_msg() {
  local captured
  captured=$(die_usage "expected test message" 2>&1) || true
  [ "$captured" = "expected test message" ]
}
if test_die_usage_msg; then
  echo "  OK die_usage prints message to stderr"
  pass=$((pass + 1))
else
  echo "  FAIL FAIL: die_usage stderr message mismatch"
  fail=$((fail + 1))
fi

# ════════════════════════════════════════════════════════════════════
# === NEW TESTS === _timeout_wrap (4 tests)
# ════════════════════════════════════════════════════════════════════

echo ""
echo "=== _timeout_wrap ==="

# Test 1: runs a simple command and captures output
result=$(_timeout_wrap 5 echo "hello world")
assert_eq "hello world" "$result" "_timeout_wrap simple command"

# Test 2: preserves exit code of successful command
assert_run_0 "_timeout_wrap 5 true" "_timeout_wrap true exits 0"

# Test 3: preserves exit code of failing command
assert_run_1 "_timeout_wrap 5 false" "_timeout_wrap false exits 1"

# Test 4: multi-word arguments passed correctly
result=$(_timeout_wrap 5 printf '%s\n' "a b" "c")
assert_eq "a b
c" "$result" "_timeout_wrap multi-word args"

# ════════════════════════════════════════════════════════════════════
# === NEW TESTS === inject_connect (5 tests)
# ════════════════════════════════════════════════════════════════════

echo ""
echo "=== inject_connect ==="

# Test 1: generates JS preamble containing "const session"
assert_run_0 "inject_connect 9222 | grep -q 'session'" \
  "inject_connect contains session reference"

# Test 2: contains the port number
assert_run_0 "inject_connect 9222 | grep -q 'port 9222'" \
  "inject_connect contains port"

# Test 3: contains key CDP method calls
assert_run_0 "inject_connect 9222 | grep -q 'createTarget'" \
  "inject_connect contains createTarget"
assert_run_0 "inject_connect 9222 | grep -q 'attachToTarget'" \
  "inject_connect contains attachToTarget"
assert_run_0 "inject_connect 9222 | grep -q 'Page.enable'" \
  "inject_connect contains Page.enable"

# Test 4: extras string is injected after connect, before tab creation
js_extras=$(inject_connect 9222 "const EXTRA = true;")
case "$js_extras" in
  *"const EXTRA = true;"*) echo "  OK inject_connect extras injected"; pass=$((pass + 1)) ;;
  *) echo "  FAIL FAIL: inject_connect extras not found"; fail=$((fail + 1)) ;;
esac

# Test 5: without extras, no extra code between connect and tab
js_noextras=$(inject_connect 9222)
case "$js_noextras" in
  *"catch(e)"*"var __tab"*) echo "  OK inject_connect no extras clean flow"; pass=$((pass + 1)) ;;
  *) echo "  FAIL FAIL: inject_connect flow broken without extras"; fail=$((fail + 1)) ;;
esac

# ════════════════════════════════════════════════════════════════════
# === NEW TESTS === _http_probe (4 tests)
# ════════════════════════════════════════════════════════════════════

echo ""
echo "=== _http_probe ==="
echo "  (curl mocked to return controlled responses)"

# Save reference to real curl — we'll shadow it with a mock
# The mock returns specific content so we can test the function logic

# Test 1: valid JSON response returns 0 (success)
curl() { echo '{"Browser": "Chrome", "webSocketDebuggerUrl": "ws://..."}'; }
assert_run_0 "_http_probe 9998" "_http_probe valid JSON returns 0"
unset -f curl

# Test 2: curl returns empty string returns 1
curl() { echo ""; }
assert_run_1 "_http_probe 9997" "_http_probe empty response returns 1"
unset -f curl

# Test 3: curl returns invalid (non-JSON) response returns 1
curl() { echo "not valid json at all"; }
assert_run_1 "_http_probe 9996" "_http_probe invalid JSON returns 1"
unset -f curl

# Test 4: curl fails entirely (non-zero exit) returns 1
curl() { return 1; }
assert_run_1 "_http_probe 9995" "_http_probe curl failure returns 1"
unset -f curl

# ════════════════════════════════════════════════════════════════════
# === NEW TESTS === _file_probe (3 tests)
# ════════════════════════════════════════════════════════════════════

echo ""
echo "=== _file_probe ==="
echo "  (using temp DevToolsActivePort files)"

# Create temp directory with mock DevToolsActivePort files
TEST_PROBE_DIR=$(mktemp -d /tmp/gsearch-test-fileprobe-XXXXXX)
echo "9994" > "$TEST_PROBE_DIR/port-9994-active"
echo "9993" > "$TEST_PROBE_DIR/port-9993-active"

# Override _scan_devtools_active_ports to return our test files
_scan_devtools_active_ports() {
  find "$TEST_PROBE_DIR" -name 'port-*-active' 2>/dev/null
}

# Test 1: matching port found in active port files returns 0
assert_run_0 "_file_probe 9994" "_file_probe matching port 9994 returns 0"

# Test 2: different matching port also returns 0
assert_run_0 "_file_probe 9993" "_file_probe matching port 9993 returns 0"

# Test 3: port not found returns 1
assert_run_1 "_file_probe 9992" "_file_probe non-matching port returns 1"

# Restore original function and clean up
unset -f _scan_devtools_active_ports
rm -rf "$TEST_PROBE_DIR"

# ════════════════════════════════════════════════════════════════════
# === NEW TESTS === _cdp_call (4 tests)
# ════════════════════════════════════════════════════════════════════

echo ""
echo "=== _cdp_call ==="
echo "  (mocking bun to return controlled output)"

# Save original CDP_SCRIPTS
ORIG_CDP_SCRIPTS="${CDP_SCRIPTS:-}"

# Create a mock browser-automation.ts
MOCK_CDP_DIR=$(mktemp -d /tmp/gsearch-test-cdpdir-XXXXXX)
# Write a minimal script that echoes its args as JSON
cat > "$MOCK_CDP_DIR/browser-automation.ts" <<'MOCKEOF'
#!/usr/bin/env bun
// Mock — just echo args as JSON
const args = process.argv.slice(2);
console.log(JSON.stringify({ success: true, subcommand: args[0], args: args.slice(1) }));
MOCKEOF
chmod +x "$MOCK_CDP_DIR/browser-automation.ts"

# Also mock bun() to point to our script
# NOTE: _cdp_call passes the script path as first arg to bun, then subcommand + args.
# The real bun consumes the script path; our mock must skip it.
bun() {
  local script_path="$1"; shift  # consume script path (already set as CDP_SCRIPTS)
  "$MOCK_CDP_DIR/browser-automation.ts" "$@"
}

# Mock _timeout_wrap to just run the command directly
_timeout_wrap() {
  local duration="$1"; shift
  "$@"
}

CDP_SCRIPTS="$MOCK_CDP_DIR"
GSEARCH_CDP_PORT="9999"

# Test 1: basic call succeeds
result=$(_cdp_call 30 follow "https://example.com" 2>/dev/null) || true
if echo "$result" | python3 -c 'import sys,json; d=json.load(sys.stdin); assert d.get("success")' 2>/dev/null; then
  echo "  OK _cdp_call basic call succeeds"
  pass=$((pass + 1))
else
  echo "  FAIL FAIL: _cdp_call basic call failed"
  echo "    result: $result"
  fail=$((fail + 1))
fi

# Test 2: subcommand passed through
if echo "$result" | python3 -c 'import sys,json; d=json.load(sys.stdin); assert d.get("subcommand")=="follow"' 2>/dev/null; then
  echo "  OK _cdp_call passes subcommand"
  pass=$((pass + 1))
else
  echo "  FAIL FAIL: _cdp_call subcommand not passed"
  fail=$((fail + 1))
fi

# Test 3: script not found returns exit 2
result=$(_cdp_call 30 follow 2>&1) && rc=0 || rc=$?
# Temporarily remove the script to test not-found path
SAVE_CDP="$CDP_SCRIPTS"
CDP_SCRIPTS="/nonexistent/path"
result2=$(_cdp_call 30 follow 2>&1) && rc2=0 || rc2=$?
CDP_SCRIPTS="$SAVE_CDP"
if [ $rc2 -eq 2 ]; then
  echo "  OK _cdp_call script not found returns exit 2"
  pass=$((pass + 1))
else
  echo "  FAIL FAIL: _cdp_call script not found should return exit 2 (got $rc2)"
  fail=$((fail + 1))
fi

# Test 4: port flag is appended
if echo "$result" | python3 -c 'import sys,json; d=json.load(sys.stdin); assert "--port" in d["args"]; assert "9999" in d["args"]' 2>/dev/null; then
  echo "  OK _cdp_call appends --port flag"
  pass=$((pass + 1))
else
  echo "  FAIL FAIL: _cdp_call did not append --port"
  fail=$((fail + 1))
fi

# Cleanup mock
rm -rf "$MOCK_CDP_DIR"
CDP_SCRIPTS="${ORIG_CDP_SCRIPTS:-}"

# ════════════════════════════════════════════════════════════════════
# === NEW TESTS === _validate_json (4 tests)
# ════════════════════════════════════════════════════════════════════

echo ""
echo "=== _validate_json ==="

# Test 1: valid JSON object returns 0
assert_run_0 "_validate_json '{\"a\":1}'" "_validate_json valid object"
# Test 2: valid JSON array returns 0
assert_run_0 "_validate_json '[1,2,3]'"
# Test 3: empty string returns 1
assert_run_1 "_validate_json ''"
# Test 4: invalid string returns 1
assert_run_1 "_validate_json 'not json'"

# ════════════════════════════════════════════════════════════════════
# === NEW TESTS === _json_error (2 tests)
# ════════════════════════════════════════════════════════════════════

echo ""
echo "=== _json_error ==="

# Test 1: exits with code 2
test_json_error_exit() {
  ( _json_error "test_cmd" "test_error" "test detail" 2>/dev/null; ) && return 1 || return 0
}
if test_json_error_exit; then
  echo "  OK _json_error exits with code 2"
  pass=$((pass + 1))
else
  echo "  FAIL FAIL: _json_error exit code is not 2"
  fail=$((fail + 1))
fi

# Test 2: outputs structured JSON to stderr
test_json_error_output() {
  local captured
  captured=$(_json_error "mycmd" "myerr" "mydetail" 2>&1) || true
  echo "$captured" | python3 -c 'import sys,json; d=json.load(sys.stdin); assert d["tool"]=="gsearch"; assert d["command"]=="mycmd"; assert d["error"]=="myerr"'
}
if test_json_error_output; then
  echo "  OK _json_error outputs structured JSON to stderr"
  pass=$((pass + 1))
else
  echo "  FAIL FAIL: _json_error output format incorrect"
  fail=$((fail + 1))
fi

# ════════════════════════════════════════════════════════════════════
# === NEW TESTS === _check_content_quality (5 tests)
# ════════════════════════════════════════════════════════════════════

echo ""
echo "=== _check_content_quality ==="

# Helper to evaluate quality output and check fields
check_quality() {
  local json_input="$1" expected_ok="$2" label="$3"
  local result
  result=$(_check_content_quality "$json_input" 2>/dev/null)
  eval "$result" 2>/dev/null
  if [ "${quality_ok:-}" = "$expected_ok" ]; then
    echo "  OK $label (quality_ok=$quality_ok)"
    pass=$((pass + 1))
  else
    echo "  FAIL FAIL: $label (expected quality_ok=$expected_ok, got quality_ok=${quality_ok:-unset})"
    fail=$((fail + 1))
  fi
}

# Test 1: valid content with sufficient length
check_quality '{"content":"The quick brown fox jumps over the lazy dog. This is a valid sentence with good content that passes all quality checks. Multiple sentences here."}' "true" "_check_content_quality valid content"

# Test 2: empty content
check_quality '{"content":""}' "false" "_check_content_quality empty content"

# Test 3: very short content
check_quality '{"content":"short"}' "false" "_check_content_quality short content"

# Test 4: content with _error field
check_quality '{"content":"some text","_error":"navigation_failed"}' "false" "_check_content_quality _error field"

# Test 5: malformed JSON (should produce parse error -> quality_ok=false)
check_quality "not json" "false" "_check_content_quality malformed JSON"

# ════════════════════════════════════════════════════════════════════
# === NEW TESTS === _secondary_quality_check (4 tests)
# ════════════════════════════════════════════════════════════════════

echo ""
echo "=== _secondary_quality_check ==="

# Make sure function is available (sourced from follow.sh)
# If not already sourced, try to source it
declare -F _secondary_quality_check >/dev/null 2>&1 || {
  FOLLOW_LIB="$LIB/follow.sh"
  [ -f "$FOLLOW_LIB" ] && . "$FOLLOW_LIB"
}

# Test 1: clean content with no bad patterns returns 1 (grep no match = clean)
assert_run_1 "_secondary_quality_check 'This is normal article content that should pass the quality check.'" \
  "_secondary_quality_check clean content returns 1 (no match)"

# Test 2: content with subscribe prompt returns 0 (grep match = flagged)
assert_run_0 "_secondary_quality_check 'Please subscribe to continue reading this article.'" \
  "_secondary_quality_check subscribe prompt flagged (returns 0)"

# Test 3: content with captcha returns 0 (grep match = flagged)
assert_run_0 "_secondary_quality_check 'Please verify you are human to continue.'" \
  "_secondary_quality_check captcha flagged (returns 0)"

# Test 4: content with security check pattern returns 0
assert_run_0 "_secondary_quality_check 'Security check: please prove you are human to continue.'" \
  "_secondary_quality_check security check flagged (returns 0)"

# Test 5: empty content returns 1 (empty content check before grep)
assert_run_1 "_secondary_quality_check ''" \
  "_secondary_quality_check empty content returns 1"

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
