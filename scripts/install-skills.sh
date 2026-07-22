#!/bin/bash
# Install gthings skill to global agent skills directory.
# This makes AI agent instructions available to any project.
#
# Usage: ./scripts/install-skills.sh [--prefix ~/.agents]

set -euo pipefail

SKILL_SOURCE="$(cd "$(dirname "$0")/../skills/gthings" && pwd)"
PREFIX="${1:-$HOME/.agents}"
SKILL_DEST="$PREFIX/skills/gthings"

if [ ! -d "$SKILL_SOURCE" ]; then
    echo "Error: skill source not found at $SKILL_SOURCE"
    echo "Run this script from the project root or specify a custom --prefix"
    exit 1
fi

mkdir -p "$SKILL_DEST/reference"

cp "$SKILL_SOURCE/SKILL.md" "$SKILL_DEST/SKILL.md"
cp "$SKILL_SOURCE/reference/"*.md "$SKILL_DEST/reference/"

echo "Installed gthings skill to $SKILL_DEST"
echo ""
echo "Files:"
ls -1 "$SKILL_DEST/SKILL.md" "$SKILL_DEST/reference/"*.md
