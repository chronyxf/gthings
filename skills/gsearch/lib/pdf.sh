# gsearch pdftotext — PDF text extraction via Bun/TypeScript
# shellcheck disable=all

# ===== PDFTOTEXT =====

# cmd_pdftotext: Download PDF and extract text using pdf-extract.ts.
# Uses curl for download (with retry) and TypeScript for text extraction.
# Caches extracted text to /tmp/nyx-search-cache/ for repeat access.
cmd_pdftotext() {
  # Usage check — exit code 1 for usage errors
  [ $# -ge 1 ] || { printf '{"tool":"gsearch","command":"pdftotext","error":"usage","detail":"Usage: gsearch pdftotext <pdf-url>"}\n' >&2; exit 1; }
  local url="$1"; shift
  case "$url" in *://*) ;; *) url="https://$url" ;; esac

  # ═══════════════════════════════════════════════════════════════════
  # Stage 1: arXiv URL rewrite
  # ═══════════════════════════════════════════════════════════════════
  # Convert /pdf/ to /abs/ for HTML abstract extraction
  url=$(_rewrite_arxiv_url "$url")

  # ═══════════════════════════════════════════════════════════════════
  # Stage 2: Content cache check
  # ═══════════════════════════════════════════════════════════════════
  # Check /tmp/nyx-search-cache/ before downloading (same cache pattern
  # used by browser-automation.ts). Cache key is SHA-256 of the URL.
  local cache_key cache_path
  cache_key=$(printf '%s' "$url" | shasum -a 256 | cut -d' ' -f1)
  cache_path="/tmp/nyx-search-cache/$cache_key"
  if [ -f "$cache_path" ]; then
    local cached_text
    cached_text=$(cat "$cache_path" 2>/dev/null) || cached_text=""
    if [ -n "$cached_text" ]; then
      # Cache hit — return cached content directly without re-extraction
      printf '{"url":%s,"content":%s,"method":"cache","source":%s,"length":%d,"truncated":false}\n' \
        "$(json_str "$url")" "$(json_str "$cached_text")" "$(json_str "$url")" "${#cached_text}"
      return 0
    fi
  fi

  # ═══════════════════════════════════════════════════════════════════
  # Stage 3: Locate pdf-extract.ts script
  # ═══════════════════════════════════════════════════════════════════
  local script_dir extractor
  script_dir=$(cd "$(dirname "$(readlink "$0" 2>/dev/null || echo "$0")")" && pwd)
  extractor=$(find "$script_dir/.." -name 'pdf-extract.ts' 2>/dev/null | head -1)
  if [ -z "$extractor" ]; then
    extractor="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/scripts/pdf-extract.ts"
  fi

  if [ ! -f "$extractor" ]; then
    _json_error "pdftotext" "pdf_extractor_not_found" "pdf-extract.ts not found at $extractor"
  fi

  # ═══════════════════════════════════════════════════════════════════
  # Stage 4: Download PDF with reliability flags
  # ═══════════════════════════════════════════════════════════════════
  # Use --max-time 30 to cap slow responses and --retry 2 for transient failures
  local pdf_data
  pdf_data=$(curl -sL --max-time 30 --retry 2 "$url") || {
    _json_error "pdftotext" "pdf_download_failed" "Failed to download PDF from $url"
  }

  # ═══════════════════════════════════════════════════════════════════
  # Stage 5: Extract text via pdf-extract.ts (returns JSON with text field)
  # ═══════════════════════════════════════════════════════════════════
  local raw_output
  raw_output=$(printf '%s' "$pdf_data" | bun "$extractor" 2>/dev/null) || {
    local exit_code=$?
    if [ "$exit_code" -eq 1 ]; then
      _json_error "pdftotext" "pdf_extraction_failed" "No text could be extracted from PDF"
    else
      _json_error "pdftotext" "pdf_extraction_error" "Extraction process failed with exit code $exit_code"
    fi
  }

  # Parse the text field from pdf-extract.ts JSON output
  local text
  text=$(printf '%s' "$raw_output" | python3 -c 'import sys,json; print(json.load(sys.stdin).get("text",""))' 2>/dev/null) || text=""
  if [ -z "$text" ]; then
    _json_error "pdftotext" "pdf_extraction_failed" "No text could be extracted from PDF"
  fi

  # ═══════════════════════════════════════════════════════════════════
  # Stage 6: Save raw text to content cache
  # ═══════════════════════════════════════════════════════════════════
  mkdir -p /tmp/nyx-search-cache 2>/dev/null || true
  printf '%s' "$text" > "$cache_path" 2>/dev/null || true

  # ═══════════════════════════════════════════════════════════════════
  # Stage 7: Output JSON with traceability metadata
  # ═══════════════════════════════════════════════════════════════════
  local text_length
  text_length=$(printf '%s' "$text" | wc -c | tr -d ' ')
  printf '{"url":%s,"content":%s,"method":"pdftotext","source":%s,"length":%d,"truncated":false}\n' \
    "$(json_str "$url")" "$(json_str "$text")" "$(json_str "$url")" "$text_length"
}
