#!/usr/bin/env bash
# Test: gsearch SKILL.md frontmatter and trigger section correctness.
# This validates that the skill registers properly in the opencode system.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SKILL_MD="$SCRIPT_DIR/../SKILL.md"

pass=0; fail=0
assert_contains() {
  local file="$1" pattern="$2" label="$3"
  if grep -q "$pattern" "$file"; then
    pass=$((pass+1))
  else
    echo "FAIL: $label — pattern '$pattern' not found in $file"
    fail=$((fail+1))
  fi
}

assert_contains "$SKILL_MD" "^name: gsearch" "frontmatter has name: gsearch"
assert_contains "$SKILL_MD" "Search Google" "description mentions Search Google"
assert_contains "$SKILL_MD" "read web pages" "description mentions read web pages"
assert_contains "$SKILL_MD" "extract PDFs" "description mentions extract PDFs"
assert_contains "$SKILL_MD" "Use ONLY when" "description has use-only-when gate"
assert_contains "$SKILL_MD" "When This Skill Activates" "has trigger section"
assert_contains "$SKILL_MD" '"/gsearch"' "trigger lists /gsearch phrase"
assert_contains "$SKILL_MD" "search for" "trigger lists 'search for' phrase"
assert_contains "$SKILL_MD" "Do NOT use.*webfetch" "trigger explicitly bans webfetch"
assert_contains "$SKILL_MD" "Trap 7:" "has fetch-tool-fallback trap"
assert_contains "$SKILL_MD" "webfetch sends a raw HTTP request" "trap explains why webfetch fails"

echo ""
echo "PASS: $pass/$((pass+fail)) tests passed"
exit $fail
