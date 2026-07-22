# gsearch — generic Chromium browser automation for AI agents.
# shellcheck disable=all
# Principles:
#  - No hardcoded browser names/paths (detect from env/processes)
#  - Profile dir read from process args, not hardcoded paths
#  - Every config has env override

# ===== COMMUNICATION PROTOCOL =====
#
# Shell → TypeScript (browser-automation.ts) communication:
#   Stdout = JSON payload only  — no logging, no status messages
#   Stderr = Diagnostics only   — warnings, errors, progress info
#   Exit 0  = Success           — partial failures described in JSON
#   Exit 1  = Usage error       — bad args, unknown command
#   Exit 2  = Operational error — no browser, port in use, dependency missing
#
# Every call to browser-automation.ts follows this pattern:
#   result=$(_cdp_call <timeout> <subcommand> <args>) || {
#     rc=$?; _json_error "<cmd>" "cdp_failed" "Exit $rc"
#   }
#   echo "$result" | _validate_json || ...

# ===== CONFIG (all overridable via env) =====

GSEARCH_CDP_PORT="${GSEARCH_CDP_PORT:-9222}"
GSEARCH_TOKEN_DIR="${GSEARCH_TOKEN_DIR:-/tmp/gsearch-tokens}"
GSEARCH_PROFILE_DIR="${GSEARCH_PROFILE_DIR:-/tmp/gsearch-profile}"
CHROME_PATH="${CHROME_PATH:-}"

# Rate limiting: delay in seconds between batch API calls
GSEARCH_BATCH_DELAY="${GSEARCH_BATCH_DELAY:-1}"
# Max parallel tabs per batch operation
GSEARCH_MAX_TABS="${GSEARCH_MAX_TABS:-8}"

# ===== STANDARDIZED CDP CALL =====

# _cdp_call: Standardized call to browser-automation.ts.
# Usage: result=$(_cdp_call <timeout_seconds> <subcommand> [args...]) || exit=$?
#   Stdout: JSON payload only (capture with $())
#   Stderr: Diagnostics passthrough (bun runtime errors, TS warnings)
#   Exit 0  = command succeeded (JSON payload on stdout)
#   Exit 1  = usage error from browser-automation.ts
#   Exit 2  = operational error (script not found, timeout)
_cdp_call() {
  local timeout="$1"; shift
  local sub="$1"; shift
  local script="${CDP_SCRIPTS}/browser-automation.ts"

  [ -f "$script" ] || {
    printf '{"tool":"gsearch","error":"cdp_script_not_found","detail":"browser-automation.ts not found at %s"}\n' "$script" >&2
    return 2
  }

  local rc=0
  # No 2>&1 — stdout is JSON payload, stderr is diagnostics
  _timeout_wrap "$timeout" bun "$script" "$sub" "$@" --port "$GSEARCH_CDP_PORT" || rc=$?

  # timeout(1) from coreutils returns 124 when killed
  if [ $rc -eq 124 ]; then
    printf '{"tool":"gsearch","error":"timeout","detail":"CDP operation timed out after %s seconds"}\n' "$timeout" >&2
    return 2
  fi

  return $rc
}

# ===== JSON HELPERS =====

# _validate_json: Verify a string is valid JSON. Returns 0 if valid.
_validate_json() {
  printf '%s' "$1" | python3 -c 'import sys,json; json.load(sys.stdin)' 2>/dev/null
}

# _json_error: Print structured JSON error to stderr and exit.
# Format: {"tool":"gsearch","command":"<cmd>","error":"<code>","detail":"<msg>"}
# Exits with code 2 (operational error).
_json_error() {
  local cmd="$1" code="$2" detail="$3"
  printf '{"tool":"gsearch","command":"%s","error":"%s","detail":%s}\n' "$cmd" "$code" "$(json_str "$detail")" >&2
  exit 2
}

# ===== CONTENT QUALITY =====

# _check_content_quality: Parse JSON output and check content quality.
# Returns structured quality info. Usage:
#   eval "$(_check_content_quality "$json_output")"
# Sets: quality_ok=true/false, quality_reason="..."
_check_content_quality() {
  local json="$1"
  python3 -c '
import sys, json
try:
    d = json.load(sys.stdin)
    # Check _error field
    err = d.get("_error", "") if isinstance(d, dict) else ""
    if err and err != "low_quality_content":
        print("quality_ok=false")
        print("quality_reason=" + json.dumps(err))
    elif isinstance(d, dict) and d.get("content", "") and len(d["content"]) < 80:
        print("quality_ok=false")
        print("quality_reason=\"too_short\"")
    elif isinstance(d, dict) and not d.get("content", ""):
        print("quality_ok=false")
        print("quality_reason=\"empty_content\"")
    else:
        print("quality_ok=true")
        print("quality_reason=\"\"")
except:
    print("quality_ok=false")
    print("quality_reason=\"parse_error\"")
' <<< "$json" 2>/dev/null
}

# ===== RATE LIMITING =====

# _batch_delay: Rate limiting between batch operations.
# Uses a timestamp file to track last call time and enforces
# GSEARCH_BATCH_DELAY seconds between calls.
_batch_delay() {
  local delay="${GSEARCH_BATCH_DELAY:-1}"
  [ "$delay" = "0" ] || [ "$delay" = "" ] && return 0
  local marker="/tmp/gsearch-batch-last"
  if [ -f "$marker" ]; then
    local last=$(cat "$marker" 2>/dev/null || echo 0)
    local now=$(python3 -c 'import time; print(int(time.time()))' 2>/dev/null || echo 0)
    local elapsed=$((now - last))
    if [ $elapsed -lt $delay ]; then
      sleep $((delay - elapsed))
    fi
  fi
  python3 -c 'import time; print(int(time.time()))' > "$marker" 2>/dev/null || true
}

# ===== TAB LIMIT SANITY CHECK =====

# _check_tab_limit: Verify number of URLs doesn't exceed GSEARCH_MAX_TABS.
# Exits with error if too many tabs requested.
_check_tab_limit() {
  local count=$#
  local max="${GSEARCH_MAX_TABS:-8}"
  if [ $count -gt $max ]; then
    _json_error "batch" "too_many_tabs" "Max ${max} tabs per batch, got ${count}. Increase GSEARCH_MAX_TABS or reduce inputs."
  fi
}

# ===== BROWSER DETECTION =====

# ════════════════════════════════════════════════════════════════════
# GENERIC BROWSER DETECTION
# ════════════════════════════════════════════════════════════════════

# Extract app display name from a browser executable path.
_app_name() {
  local path="$1"
  case "$path" in
    *"/Google Chrome.app/"*)  printf 'Google Chrome' ;;
    *"/Dia.app/"*)            printf 'Dia' ;;
    *)                        basename "$(dirname "$(dirname "$path")")" .app ;;
  esac
}

# Find DevToolsActivePort files anywhere under ~/Library/Application Support
_scan_devtools_active_ports() {
  find "$HOME/Library/Application Support" -name "DevToolsActivePort" -maxdepth 5 2>/dev/null
}

# Find the real profile directory for a browser (not our temp profile).
_real_profile_dir() {
  local target_app="$1" dir

  # If a specific browser was requested, check its path first
  case "$target_app" in
    *[Dd]ia*)        dir="$HOME/Library/Application Support/Dia/User Data" ;;
    *[Cc]hrome*)     dir="$HOME/Library/Application Support/Google/Chrome" ;;
    *)               # fallback: check Dia first, then Chrome
                     for dir in "$HOME/Library/Application Support/Dia/User Data" \
                                "$HOME/Library/Application Support/Google/Chrome"; do
                       [ -d "$dir" ] && { printf '%s' "$dir"; return 0; }
                     done ;;
  esac

  if [ -d "$dir" ]; then
    printf '%s' "$dir"
    return 0
  fi
  return 1
}

# ===== PORT PROBING (parallel, browser-agnostic) =====

# ════════════════════════════════════════════════════════════════════
# PORT PROBING (parallel, browser-agnostic)
# ════════════════════════════════════════════════════════════════════

# _http_probe: Check ONE port via HTTP /json/version endpoint (fast path).
# Returns 0 if browser responds with valid JSON metadata.
_http_probe() {
  local port="$1" resp ep
  # Try multiple endpoints — Dia Browser returns 404 on /json/version
  for ep in "/json/version" "/json" "/json/list"; do
    resp=$(curl -sS --max-time 2 "http://127.0.0.1:$port$ep" 2>/dev/null) || continue
    if [ -n "$resp" ] && printf '%s' "$resp" | python3 -c 'import sys,json; json.load(sys.stdin)' 2>/dev/null; then
      return 0
    fi
  done
  return 1
}

# _file_probe: Check ONE port via DevToolsActivePort files (discovery fallback).
# Returns 0 if any DevToolsActivePort on the system has this port number.
_file_probe() {
  local port="$1" pf
  while IFS= read -r pf; do
    [ -f "$pf" ] || continue
    [ "$(head -1 "$pf" 2>/dev/null)" = "$port" ] || continue
    # Verify port is actually live with TCP connect — stale files persist after browser exit
    if command -v lsof >/dev/null 2>&1; then
      lsof -ti:"$port" >/dev/null 2>&1 || continue
    else
      (echo >/dev/tcp/127.0.0.1/"$port") 2>/dev/null || continue
    fi
    return 0
  done < <(_scan_devtools_active_ports)
  return 1
}

# _probe_one: Check ONE port — runs HTTP probe and DevToolsActivePort
# file scan CONCURRENTLY (background subshells). Returns port on success.
# Uses a marker file written by whichever check succeeds first.
# Both probes have their own internal timeout bounds.
_probe_one() {
  local port="$1" _marker
  _marker=$(mktemp /tmp/gsearch-pm-XXXXXX 2>/dev/null) || _marker="/tmp/gsearch-pm-${$}-${port}"

  # Concurrent: HTTP /json/version (fast path, 2s curl timeout)
  { _http_probe "$port" && echo "1" > "$_marker" 2>/dev/null; } &

  # Concurrent: DevToolsActivePort file scan (filesystem-bound)
  { _file_probe "$port" && echo "1" > "$_marker" 2>/dev/null; } &

  # Wait for both probes to finish
  wait 2>/dev/null

  if [ -f "$_marker" ] && [ "$(cat "$_marker" 2>/dev/null)" = "1" ]; then
    rm -f "$_marker" 2>/dev/null
    printf '%s' "$port"
    return 0
  fi
  rm -f "$_marker" 2>/dev/null
  return 1
}

# ===== CONNECTION MANAGEMENT =====

# ════════════════════════════════════════════════════════════════════
# CONNECTION MANAGEMENT
# ════════════════════════════════════════════════════════════════════

# Check if a browser is already running on the given CDP port.
ensure_browser() {
  local port="${1:-$GSEARCH_CDP_PORT}"
  _probe_one "$port" >/dev/null 2>&1
}

# ===== LAUNCH NEW BROWSER =====

# ════════════════════════════════════════════════════════════════════
# LAUNCH NEW BROWSER (generics)
# ════════════════════════════════════════════════════════════════════

# force_browser_launch: Start a new browser instance with CDP enabled.
# Uses --window-size=1920,1080 for consistent viewport across screenshots.
# Uses --disable-sync to reduce filesystem I/O from profile sync.
# Cleans stale temp profiles older than 30 minutes (aggressive cleanup).
force_browser_launch() {
  local port="${1:-$GSEARCH_CDP_PORT}" chrome profile="" real_profile app_name
  chrome=$(_find_browser)
  [ -z "$chrome" ] && { printf '{"tool":"gsearch","error":"no_browser","detail":"No browser found"}\n' >&2; exit 2; }

  # Always prefer real profile if it exists
  app_name=$(_app_name "$chrome")
  real_profile=$(_real_profile_dir "$app_name")
  if [ -n "$real_profile" ]; then
    profile="$real_profile"
  fi
  # Only use temp profile as last resort — no real profile found at all
  if [ -z "$profile" ]; then
    profile="${GSEARCH_PROFILE_DIR:-/tmp/gsearch-profile}"
    _pre_seed_profile "$profile"
  fi

  rm -f "$profile/SingletonLock" "$profile/SingletonSocket" "$profile/SingletonCookie" "$profile/DevToolsActivePort" 2>/dev/null
  # Aggressive temp profile cleanup: 30 min instead of 60 min
  find /tmp -maxdepth 1 -name 'gsearch-profile-*' -mmin +30 -exec rm -rf {} + 2>/dev/null || true
  find /tmp -maxdepth 1 -name 'nyx-profile-*' -mmin +30 -exec rm -rf {} + 2>/dev/null || true
  mkdir -p "$profile"

  # --window-size=1920,1080 ensures consistent viewport for screenshots and extraction
  # --disable-sync reduces disk I/O from Chrome profile sync service
  "$chrome" --remote-debugging-port="$port" --user-data-dir="$profile" \
    --no-first-run --no-default-browser-check --disable-fre \
    --window-size=1920,1080 --disable-sync "about:blank" >/dev/null 2>&1 &
  local pid=$! i

  for i in $(seq 1 60); do
    sleep 0.2
    _probe_one "$port" >/dev/null && break
  done

  if _probe_one "$port" >/dev/null; then
    printf '{"success":true,"pid":%d,"profile":"%s","port":%s}\n' "$pid" "$profile" "$port"
    return 0
  fi
  printf '{"tool":"gsearch","error":"launch_failed","detail":"Browser failed to start on port %s"}\n' "$port" >&2
  return 1
}

_pre_seed_profile() {
  local dir="$1" now
  now=$(python3 -c 'from datetime import timezone, datetime; print(int(datetime.now(timezone.utc).timestamp()*1000000))' 2>/dev/null || echo "1")
  for sub in "User Data/Default" "Default"; do
    mkdir -p "$dir/$sub"
    cat > "$dir/$sub/Preferences" <<PEOF
{"browser":{"has_seen_welcome_page":true},"profile":{"exit_type":"Normal"},"default_apps_install_state":3,"in_product_help":{"session_last_active_time":"$now","session_number":5,"session_start_time":"$now"}}
PEOF
  done
  for sub in "User Data" "."; do
    mkdir -p "$dir/$sub"
    cat > "$dir/$sub/Local State" <<LEOF
{"browser":{"enabled_labs_experiments":[],"last_redirect_origin":"","last_whats_new_milestone":"150"},"distribution":{"skip_first_run_ui":true}}
LEOF
  done
}

# ===== BROWSER BINARY DETECTION =====

# ════════════════════════════════════════════════════════════════════
# BROWSER BINARY DETECTION
# ════════════════════════════════════════════════════════════════════

_find_browser() {
  [ -n "${CHROME_PATH:-}" ] && [ -x "$CHROME_PATH" ] && { printf '%s' "$CHROME_PATH"; return 0; }
  _detect_default_chrome && return 0
  _detect_chrome && return 0
  _install_chrome && return 0
  return 1
}

# ===== DEPENDENCY: browser-harness-js =====

# ════════════════════════════════════════════════════════════════════
# DEPENDENCY: browser-harness-js
# ════════════════════════════════════════════════════════════════════

ensure_bhjs() {
  command -v browser-harness-js >/dev/null 2>&1 && return 0
  local bhjs="$HOME/.agents/skills/cdp/sdk/browser-harness-js"
  [ -f "$bhjs" ] || { printf '{"tool":"gsearch","error":"no_bhjs","detail":"browser-harness-js not found","hint":"Install cdp skill first"}\n' >&2; exit 2; }
  mkdir -p "$HOME/.local/bin"
  ln -sf "$bhjs" "$HOME/.local/bin/browser-harness-js"
  export PATH="$HOME/.local/bin:$PATH"
}

# ===== I/O BOUNDARY: TIMEOUT WRAPPER =====

# ════════════════════════════════════════════════════════════════════
# TIMEOUT WRAPPER (OS-level safety net for hung processes)
# ════════════════════════════════════════════════════════════════════

# _timeout_wrap: Run a command with an OS-level timeout if the `timeout`
# command is available. Falls back to direct execution if not.
# This is a safety net — browser-automation.ts already has internal timeouts.
# Exit code 124 indicates timeout killed the process.
_timeout_wrap() {
  local duration="$1"; shift
  if command -v timeout >/dev/null 2>&1; then
    timeout "$duration" "$@"
  else
    "$@"
  fi
}

# ===== JS INJECTION (browser-agnostic connect) =====

# ════════════════════════════════════════════════════════════════════
# JS INJECTION (browser-agnostic connect)
# ════════════════════════════════════════════════════════════════════

# inject_connect: Generate the JS preamble string for browser-harness-js.
# Creates a CDP connection on the given port, opens a background tab,
# attaches to it, and enables Page domain.
#
# Usage: inject_connect <port> [<extras>]
#   - port: CDP port the browser is listening on
#   - extras: optional JS string injected after connect, before tab creation
#
# Reduces duplication across follow.sh (screenshot, scrape) and any module
# that calls browser-harness-js directly.
inject_connect() {
  local port="$1" extras="${2:-}"
  cat <<EOF
if (!session.isConnected()) {
  try { await session.connect({port: $port}); }
  catch(e) { throw new Error("Cannot connect on port $port"); }
}
$extras
var __tab = await session.Target.createTarget({url:"about:blank",background:true});
await session.Target.attachToTarget({targetId:__tab.targetId,flatten:true});
await session.use(__tab.targetId);
await session.Page.enable();
await session.Page.setLifecycleEventsEnabled({enabled:true});
EOF
}

# ===== HELPERS =====

# ════════════════════════════════════════════════════════════════════
# HELPERS
# ════════════════════════════════════════════════════════════════════

die_usage()  { printf '%s\n' "$1" >&2; exit 1; }
is_num()     { case "$1" in ''|*[!0-9]*) return 1;; *) return 0;; esac }
json_str()   { printf '%s' "$1" | node -e 'process.stdout.write(JSON.stringify(require("fs").readFileSync(0,"utf8").trim()))'; }

# ===== CHROMIUM BINARY DETECTION (macOS + Linux) =====

# ════════════════════════════════════════════════════════════════════
# CHROMIUM BINARY DETECTION (macOS + Linux)
# ════════════════════════════════════════════════════════════════════

_is_chromium() {
  local bin="$1" _tmp_chk
  [ -z "$bin" ] && return 1
  _tmp_chk=$(mktemp /tmp/gsearch-chk-XXXXXX 2>/dev/null) || _tmp_chk="/tmp/gsearch-chk-$$"
  strings "$bin" 2>/dev/null > "$_tmp_chk"
  grep -q "remote-debugging-port" "$_tmp_chk" 2>/dev/null
  local ret=$?; rm -f "$_tmp_chk"; return $ret
}

_detect_default_chrome() {
  local bundle_id="" app_path=""
  if [ -f "$HOME/Library/Preferences/com.apple.LaunchServices/com.apple.launchservices.secure.plist" ]; then
    bundle_id=$(python3 -c "
import plistlib, os
path = os.path.expanduser('~/Library/Preferences/com.apple.LaunchServices/com.apple.launchservices.secure.plist')
try:
    pl = plistlib.load(open(path, 'rb'))
    for h in pl.get('LSHandlers', []):
        if h.get('LSHandlerURLScheme') == 'https':
            print(h.get('LSHandlerRoleAll', ''))
            break
except: pass
" 2>/dev/null)
    if [ -n "$bundle_id" ]; then
      app_path=$(mdfind "kMDItemCFBundleIdentifier == '$bundle_id'" 2>/dev/null | head -1)
      if [ -n "$app_path" ]; then
        local bin="$app_path/Contents/MacOS/$(basename "$app_path" .app)"
        [ -x "$bin" ] || bin=$(find "$app_path/Contents/MacOS" -maxdepth 1 -type f -perm +111 2>/dev/null | head -1)
        [ -n "$bin" ] && [ -x "$bin" ] && _is_chromium "$bin" && { printf '%s' "$bin"; return 0; }
      fi
    fi
  fi
  return 1
}

_detect_chrome() {
  local c browsers=(
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
    "/Applications/Dia.app/Contents/MacOS/Dia"
    "$HOME/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
  )
  for c in "${browsers[@]}"; do [ -x "$c" ] && { printf '%s' "$c"; return 0; }; done
  if command -v mdfind >/dev/null 2>&1; then
    while IFS= read -r app; do
      name="$(basename "$app" .app)"
      bin="$app/Contents/MacOS/$name"
      [ -x "$bin" ] || bin="$app/Contents/MacOS/$(basename "$app" .app | tr '[:upper:]' '[:lower:]')"
      [ -x "$bin" ] || bin=$(find "$app/Contents/MacOS" -maxdepth 1 -type f -perm +111 2>/dev/null | head -1)
      [ -n "$bin" ] && [ -x "$bin" ] && _is_chromium "$bin" && { printf '%s' "$bin"; return 0; }
    done < <(mdfind "kMDItemKind == 'Application'" 2>/dev/null | head -100)
  fi
  c=$(command -v chrome google-chrome 2>/dev/null | head -1)
  [ -n "$c" ] && [ -x "$c" ] && { printf '%s' "$c"; return 0; }
  return 1
}

_install_chrome() {
  echo "Chrome not found. Installing via Homebrew..." >&2
  brew install --cask google-chrome 2>&1 || { echo "Homebrew install failed" >&2; exit 2; }
  local installed="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
  if [ -x "$installed" ]; then
    printf '%s' "$installed"
    return 0
  fi
  echo "Installed Chrome not found at expected path" >&2
  exit 2
}
