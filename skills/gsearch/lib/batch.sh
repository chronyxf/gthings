# gsearch batch — parallel/multi-tab batch operations via browser-automation.ts
# shellcheck disable=all

# ===== BATCH / PARALLEL OPERATIONS (information-theoretic multi-tab) =====

# _rewrite_arxiv_url: Normalize arXiv URLs to abstract HTML pages.
# Handles /pdf/, /abs/, .pdf suffix, and export.arxiv.org mirror.
_rewrite_arxiv_url() {
  local u="$1"
  case "$u" in
    *export.arxiv.org/*)
      # Rewrite export.arxiv.org to arxiv.org
      u="${u//export.arxiv.org/arxiv.org}"
      ;;
  esac
  case "$u" in
    *arxiv.org/pdf/*)
      # Convert PDF URL to abstract page (sed with | delimiter avoids / escaping)
      u=$(printf '%s' "$u" | sed 's|arxiv\.org/pdf/|arxiv.org/abs/|')
      # Strip .pdf suffix (handles both /pdf/ID.pdf and /abs/ID.pdf)
      u="${u%.pdf}"
      ;;
    *arxiv.org/abs/*.pdf)
      # Strip .pdf suffix from abs URLs
      u="${u%.pdf}"
      ;;
  esac
  printf '%s' "$u"
}

# ===== BATCH SEARCH =====

# cmd_batch_search: Multiple queries in parallel tabs, dedup by URL, sort by density.
cmd_batch_search() {
  local count=5
  while [ $# -gt 0 ]; do
    case "$1" in
      --count) is_num "$2" || die_usage "gsearch batch search: --count must be a number"; count="$2"; shift 2 ;;
      --) shift; break ;;
      -*) die_usage "gsearch batch search: unknown option: $1" ;;
      *) break ;;
    esac
  done
  [ $# -ge 1 ] || die_usage "Usage: gsearch batch search [--count N] query1 query2 ..."

  # Rate limiting between batch operations
  _batch_delay

  local raw rc=0
  raw=$(_cdp_call 45 batch-search "$@" --count "$count") || rc=$?

  if [ $rc -eq 1 ]; then
    _json_error "batch_search" "usage_error" "$raw"
  fi
  if [ $rc -eq 2 ]; then
    exit 2
  fi

  printf '%s\n' "$raw"
}

# ===== BATCH FOLLOW =====

# cmd_batch_follow: Multiple URLs in parallel tabs, returns combined content.
cmd_batch_follow() {
  local selector="article, main, [role=main]" raw=false pretty=false offset=0 max=15000
  while [ $# -gt 0 ]; do
    case "$1" in
      --selector) [ $# -ge 2 ] || die_usage; selector="$2"; shift 2 ;;
      --offset)   [ $# -ge 2 ] || die_usage; is_num "$2" || die_usage; offset="$2"; shift 2 ;;
      --max)      [ $# -ge 2 ] || die_usage; is_num "$2" || die_usage; max="$2"; shift 2 ;;
      --pretty)   pretty=true; shift ;;
      --raw) raw=true; shift ;;
      --) shift; break ;;
      -*) die_usage "gsearch batch follow: unknown option: $1" ;;
      *) break ;;
    esac
  done
  [ $# -ge 1 ] || die_usage "Usage: gsearch batch follow [--selector S] [--offset N] [--max M] [--pretty] url1 url2 ..."

  # Tab limit sanity check
  _check_tab_limit "$@"

  # Rewrite arXiv PDF URLs to abstract pages before passing to browser-automation.ts
  local urls=()
  local u
  for u in "$@"; do
    urls+=("$(_rewrite_arxiv_url "$u")")
  done

  # Rate limiting between batch operations
  _batch_delay

  local raw_out rc=0
  raw_out=$(_cdp_call 45 batch-follow "${urls[@]}" \
    --selector "$selector" --offset "$offset" --max "$max" --timeout 15000 \
    $($pretty && echo --pretty)) || rc=$?

  if [ $rc -eq 1 ]; then
    _json_error "batch_follow" "usage_error" "$raw_out"
  fi
  if [ $rc -eq 2 ]; then
    exit 2
  fi

  printf '%s\n' "$raw_out"
}

# ===== BATCH HARVEST =====

# cmd_batch_harvest: Full information pipeline.
#   Phase 1: parallel search (all queries → dedup → rank by snippet length)
#   Phase 2: parallel follow (top N unique URLs → extract content)
cmd_batch_harvest() {
  local count=5 max_pages=5 max_per_topic=5
  while [ $# -gt 0 ]; do
    case "$1" in
      --count) is_num "$2" || die_usage; count="$2"; shift 2 ;;
      --max)   is_num "$2" || die_usage; max_pages="$2"; shift 2 ;;
      --maxN)  is_num "$2" || die_usage; max_per_topic="$2"; shift 2 ;;
      --topics) shift ;;  # remaining positional args are topics (already the default)
      --) shift; break ;;
      -*) die_usage "gsearch batch harvest: unknown option: $1" ;;
      *) break ;;
    esac
  done
  [ $# -ge 1 ] || die_usage "Usage: gsearch batch harvest [--count N] [--max M] [--maxN N] [--topics] query1 query2 ..."

  # Rate limiting between batch operations
  _batch_delay

  local raw rc=0
  raw=$(_cdp_call 45 batch-harvest "$@" --count "$count" --max "$max_pages" --timeout 15000) || rc=$?

  if [ $rc -eq 1 ]; then
    _json_error "batch_harvest" "usage_error" "$raw"
  fi
  if [ $rc -eq 2 ]; then
    exit 2
  fi

  printf '%s\n' "$raw"
}

# ===== BATCH DISPATCHER =====

# cmd_batch: Dispatch to subcommands with pre-flight check.
cmd_batch() {
  [ $# -ge 1 ] || die_usage "Usage: gsearch batch <search|follow|harvest> [args...]"
  local sub="$1"; shift

  # Pre-flight: ensure browser is available before expensive batch ops
  ensure_browser || force_browser_launch "$GSEARCH_CDP_PORT" || {
    printf '{"tool":"gsearch","command":"batch","error":"no_browser","detail":"Cannot start browser for batch operation on port %s"}\n' "$GSEARCH_CDP_PORT" >&2
    exit 2
  }

  case "$sub" in
    search)  cmd_batch_search "$@" ;;
    follow)  cmd_batch_follow "$@" ;;
    harvest) cmd_batch_harvest "$@" ;;
    *)
      printf '{"tool":"gsearch","command":"batch","error":"unknown_subcommand","detail":"Unknown subcommand: %s (use: search | follow | harvest)"}\n' "$sub" >&2
      exit 2
      ;;
  esac
}
