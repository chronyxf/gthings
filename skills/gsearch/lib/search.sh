# gsearch search — single search via browser-automation.ts
# shellcheck disable=all

# ===== SEARCH (single-query web search) =====

# _validate_search_results: Check that browser-automation.ts output contains
# valid search results (non-empty array of objects with title + url).
# Returns 0 if valid, 1 if empty or malformed.
_validate_search_results() {
  local json="$1"
  [ -z "$json" ] && return 1
  printf '%s' "$json" | python3 -c '
import sys, json
try:
    data = json.load(sys.stdin)
    if isinstance(data, list) and len(data) > 0:
        # Verify at least one result has title and url
        valid = sum(1 for r in data if r.get("title") and r.get("url"))
        if valid > 0:
            print("true")
        else:
            print("false")
    else:
        print("false")
except:
    print("false")
' 2>/dev/null | grep -q "true"
}

# cmd_search: Single-query Google search via browser-automation.ts.
# Uses the standardized _cdp_call for clean stdout/stderr separation.
# Retries once with trailing space to bypass cache on empty results.
cmd_search() {
  local pretty=false count=10
  while [ $# -gt 0 ]; do
    case "$1" in
      --pretty) pretty=true; shift ;;
      --count)  is_num "$2" || die_usage "gsearch: --count must be a number"; count="$2"; shift 2 ;;
      --) shift; break ;;
      -*) die_usage "gsearch: unknown option: $1" ;;
      *) break ;;
    esac
  done
  [ $# -ge 1 ] || die_usage "Usage: gsearch [--pretty] [--count N] <query>"
  local query="$1"

  local raw rc=0
  # First attempt using standardized CDP call (no 2>&1 — stdout=JSON, stderr=diagnostics)
  raw=$(_cdp_call 30 search "$query" --count "$count") || rc=$?

  # Handle usage error (exit 1 from browser-automation.ts)
  if [ $rc -eq 1 ]; then
    _json_error "search" "usage_error" "$raw"
  fi
  # Handle operational error (exit 2)
  if [ $rc -eq 2 ]; then
    exit 2
  fi

  # Validate results: check for non-empty array with title+url
  if ! _validate_search_results "$raw"; then
    # Retry once with a slight query variation (trailing space to bypass cache)
    sleep 1
    rc=0
    raw=$(_cdp_call 30 search "$query " --count "$count") || rc=$?

    if [ $rc -ne 0 ] || ! _validate_search_results "$raw"; then
      # Report failure with structured error JSON (distinct from empty results)
      printf '{"error":"search_failed","detail":"CDP call failed or no valid results after retry","exit_code":%d}\n' "$rc"
      return 1
    fi
  fi

  if $pretty; then
    echo "$raw" | python3 -m json.tool
  else
    printf '%s\n' "$raw"
  fi
}
