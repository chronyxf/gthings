#!/bin/bash
# Create a new changeset entry interactively.
# Usage: ./scripts/create-changeset.sh
#
# Prompts for:
#   - Description (one line summarizing the change)
#   - Crate name(s) affected
#   - Bump type(s): patch|minor|major
#
# Produces a file in .changesets/ with proper YAML frontmatter.

set -euo pipefail

CHANGESETS_DIR=".changesets"

if [ ! -d "$CHANGESETS_DIR" ]; then
    mkdir -p "$CHANGESETS_DIR"
    echo "Created $CHANGESETS_DIR/"
fi

echo "=== Create a Changeset ==="
echo ""

read -p "Short description (one line, will become filename): " description
if [ -z "$description" ]; then
    echo "Error: description required"
    exit 1
fi

# Sanitize description for filename
filename=$(echo "$description" | tr '[:upper:]' '[:lower:]' | sed 's/[^a-z0-9]/-/g' | sed 's/--*/-/g' | sed 's/^-//;s/-$//')
filename="${CHANGESETS_DIR}/${filename}.md"

# Collect crate bumps
declare -a crates=()
declare -a bumps=()

echo ""
echo "Enter crate bumps (one at a time). Leave crate name empty when done."
echo ""

while true; do
    read -p "  Crate name (or empty to finish): " crate_name
    if [ -z "$crate_name" ]; then
        break
    fi
    read -p "  Bump type (patch/minor/major): " bump_type
    case "$bump_type" in
        patch|minor|major)
            crates+=("$crate_name")
            bumps+=("$bump_type")
            ;;
        *)
            echo "  Invalid bump type. Use patch, minor, or major."
            continue
            ;;
    esac
done

if [ ${#crates[@]} -eq 0 ]; then
    echo "Error: at least one crate is required"
    exit 1
fi

# Write changeset file
{
    echo "---"
    for i in "${!crates[@]}"; do
        echo "\"${crates[$i]}\": ${bumps[$i]}"
    done
    echo "---"
    echo ""
    echo "- $description"
} > "$filename"

echo ""
echo "Created: $filename"
echo ""
echo "Contents:"
cat "$filename"
