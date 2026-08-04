#!/bin/bash
# consume-changesets.sh — list and validate changesets in .changeset/
# Helper for the VERSION.md workflow. Prints each changeset and validates
# that its YAML frontmatter matches the expected knope format.
set -euo pipefail

CHANGESETS_DIR=".changeset"
VALID_PACKAGES=("gthings-common" "gthings-extraction" "gthings-cdp" "gthings-search" "gthings")
VALID_BUMPS=("patch" "minor" "major")

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

if [ ! -d "$CHANGESETS_DIR" ]; then
    echo "[INFO] No $CHANGESETS_DIR directory found. Nothing to consume."
    exit 0
fi

shopt -s nullglob
FILES=("$CHANGESETS_DIR"/*.md)

if [ ${#FILES[@]} -eq 0 ]; then
    echo "[INFO] No changesets found in $CHANGESETS_DIR/."
    exit 0
fi

echo "[INFO] Found ${#FILES[@]} changeset(s) in $CHANGESETS_DIR/:"
echo ""

FAILED=0
for file in "${FILES[@]}"; do
    echo "--- $file ---"
    cat "$file"
    echo ""

    # Validate frontmatter: first line after '---' must be '<package>: <bump>'
    if ! head -n 1 "$file" | grep -q '^---$'; then
        echo "[WARN] $file: missing leading '---' frontmatter delimiter." >&2
        FAILED=1
        continue
    fi

    FRONT=$(sed -n '2p' "$file")
    if ! [[ "$FRONT" =~ ^([a-z0-9-]+):[[:space:]](patch|minor|major)$ ]]; then
        echo "[WARN] $file: frontmatter line 2 is not '<package>: <bump>' (got: '$FRONT')." >&2
        FAILED=1
        continue
    fi

    PKG="${BASH_REMATCH[1]}"
    BUMP="${BASH_REMATCH[2]}"

    if ! contains "$PKG" "${VALID_PACKAGES[@]}"; then
        echo "[WARN] $file: unknown package '$PKG'. Expected one of: ${VALID_PACKAGES[*]}." >&2
        FAILED=1
    fi
    if ! contains "$BUMP" "${VALID_BUMPS[@]}"; then
        echo "[WARN] $file: invalid bump '$BUMP'. Expected one of: ${VALID_BUMPS[*]}." >&2
        FAILED=1
    fi
done

if [ "$FAILED" = "1" ]; then
    echo "[FAIL] One or more changesets failed validation. See warnings above." >&2
    exit 1
fi

echo "[PASS] All changesets valid."
