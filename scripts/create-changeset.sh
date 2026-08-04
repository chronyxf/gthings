#!/bin/bash
# create-changeset.sh — interactive/CLI helper to create a .changeset/<crate>-<desc>.md file
# with YAML frontmatter (knope 0.23 compatible).
set -euo pipefail

VALID_PACKAGES=("gthings-common" "gthings-extraction" "gthings-cdp" "gthings-search" "gthings")
VALID_BUMPS=("patch" "minor" "major")
VALID_TYPES=("fix" "feat" "refactor" "chore")

CHANGESETS_DIR=".changeset"

usage() {
    cat <<'EOF'
Usage:
  scripts/create-changeset.sh [package] [bump] [description] [type]

  package     one of: gthings-common, gthings-extraction, gthings-cdp, gthings-search, gthings
  bump        one of: patch, minor, major
  description short kebab-case description (used in the file name)
  type        one of: fix, feat, refactor, chore (default: fix)

With no arguments, prompts interactively.
EOF
}

contains() {
    local needle="$1"
    shift
    for item in "$@"; do
        if [ "$item" = "$needle" ]; then
            return 0
        fi
    done
    return 1
}

kebab() {
    # Lowercase, replace non-alphanumeric runs with a single dash, trim dashes.
    echo "$1" | tr '[:upper:]' '[:lower:]' | sed -E 's/[^a-z0-9]+/-/g' | sed -E 's/^-+|-+$//g'
}

prompt() {
    local var="$1" msg="$2"
    local val
    printf '%s: ' "$msg"
    read -r val
    eval "$var=\"\$val\""
}

# --- Resolve arguments or prompt ---
PACKAGE="${1:-}"
BUMP="${2:-}"
DESCRIPTION="${3:-}"
TYPE="${4:-}"

if [ -z "$PACKAGE" ]; then
    prompt PACKAGE "Package name (${VALID_PACKAGES[*]})"
fi
if [ -z "$BUMP" ]; then
    prompt BUMP "Bump type (${VALID_BUMPS[*]})"
fi
if [ -z "$DESCRIPTION" ]; then
    prompt DESCRIPTION "Short description (kebab-case)"
fi
if [ -z "$TYPE" ]; then
    TYPE="fix"
fi

# --- Validate ---
if ! contains "$PACKAGE" "${VALID_PACKAGES[@]}"; then
    echo "[ERROR] Invalid package '$PACKAGE'. Must be one of: ${VALID_PACKAGES[*]}" >&2
    exit 1
fi
if ! contains "$BUMP" "${VALID_BUMPS[@]}"; then
    echo "[ERROR] Invalid bump '$BUMP'. Must be one of: ${VALID_BUMPS[*]}" >&2
    exit 1
fi
if ! contains "$TYPE" "${VALID_TYPES[@]}"; then
    echo "[ERROR] Invalid type '$TYPE'. Must be one of: ${VALID_TYPES[*]}" >&2
    exit 1
fi
if [ -z "$DESCRIPTION" ]; then
    echo "[ERROR] Description cannot be empty." >&2
    exit 1
fi

# --- Build file name ---
CRATE_SHORT="${PACKAGE#gthings-}"
[ "$CRATE_SHORT" = "$PACKAGE" ] && CRATE_SHORT="gthings"
DESC_KEBAB="$(kebab "$DESCRIPTION")"
FILENAME="${CRATE_SHORT}-${DESC_KEBAB}.md"
FILEPATH="${CHANGESETS_DIR}/${FILENAME}"

mkdir -p "$CHANGESETS_DIR"

cat > "$FILEPATH" <<EOF
---
$PACKAGE: $BUMP
---

- $TYPE: $DESCRIPTION
EOF

echo "[OK] Created $FILEPATH"
echo "     Frontmatter: $PACKAGE: $BUMP"
echo "     Body:        - $TYPE: $DESCRIPTION"
