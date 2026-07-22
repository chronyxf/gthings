#!/usr/bin/env bash
# Test: CDP SKILL.md frontmatter and structure correctness
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

assert_contains "$SKILL_MD" "^name: cdp" "frontmatter has name: cdp"
assert_contains "$SKILL_MD" "^description:" "has description field"
assert_contains "$SKILL_MD" "Trap 1:" "has Trap 1"
assert_contains "$SKILL_MD" "Trap 2:" "has Trap 2"
assert_contains "$SKILL_MD" "Trap 3:" "has Trap 3"
assert_contains "$SKILL_MD" "Trap 4:" "has Trap 4"
assert_contains "$SKILL_MD" "Playbook" "has Playbook section"
assert_contains "$SKILL_MD" "Decision Tree" "has Decision Tree section"
assert_contains "$SKILL_MD" "Reference Documents" "has Reference Documents table"
assert_contains "$SKILL_MD" "tests/" "references test suite"
assert_contains "$SKILL_MD" "browser-automation.ts" "mentions browser-automation.ts"

echo ""
echo "PASS: $pass/$((pass+fail)) tests passed"
exit $fail
