#!/bin/bash
# Consume changeset files and update CHANGELOG.md
# Usage: ./scripts/consume-changesets.sh
#
# Changeset file format (Markdown with YAML frontmatter):
#
#   ---
#   "crate-name": patch|minor|major
#   ---
#
#   - Bullet point description
#   - Another bullet point
#

set -euo pipefail

CHANGESETS_DIR=".changesets"
CHANGELOG="CHANGELOG.md"

if [ ! -d "$CHANGESETS_DIR" ]; then
    echo "No .changesets/ directory found"
    exit 0
fi

# Collect all changeset files
files=("$CHANGESETS_DIR"/*.md)
if [ ${#files[@]} -eq 0 ] || [ ! -f "${files[0]}" ]; then
    echo "No changeset files found in $CHANGESETS_DIR/"
    exit 0
fi

echo "Consuming ${#files[@]} changeset(s)..."

declare -a minor_entries=()
declare -a patch_entries=()
declare -a major_entries=()

for f in "${files[@]}"; do
    basename=$(basename "$f" .md)
    
    # Parse frontmatter: read lines between --- delimiters
    in_frontmatter=0
    crates=()
    bumps=()
    while IFS= read -r line; do
        if [[ "$line" == "---" ]]; then
            if [ $in_frontmatter -eq 0 ]; then
                in_frontmatter=1
            else
                break  # end of frontmatter
            fi
            continue
        fi
        if [ $in_frontmatter -eq 1 ]; then
            # Match lines like: "crate-name": patch
            if [[ "$line" =~ ^\"([^\"]+)\":[[:space:]]*(patch|minor|major)$ ]]; then
                crates+=("${BASH_REMATCH[1]}")
                bumps+=("${BASH_REMATCH[2]}")
            fi
        fi
    done < "$f"
    
    # Read description lines (after frontmatter, excluding blank lines)
    desc_lines=()
    in_desc=0
    while IFS= read -r line; do
        if [[ "$line" == "---" ]]; then
            if [ $in_desc -eq 0 ]; then
                in_desc=1
                continue
            else
                in_desc=2
                continue
            fi
        fi
        if [ $in_desc -ge 2 ]; then
            # Collect non-empty, non-heading lines
            trimmed=$(echo "$line" | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')
            if [ -n "$trimmed" ] && [[ ! "$trimmed" == \#* ]]; then
                desc_lines+=("$trimmed")
            fi
        fi
    done < "$f"
    
    # Determine the highest bump across all crates in this file
    highest_bump="patch"
    for bump in "${bumps[@]}"; do
        case "$bump" in
            major) highest_bump="major" ;;
            minor) if [ "$highest_bump" != "major" ]; then highest_bump="minor"; fi ;;
        esac
    done

    # Collect description bullets (each `- ` line is a separate entry)
    bullets=()
    for line in "${desc_lines[@]}"; do
        if [[ "$line" == "- "* ]]; then
            bullets+=("$line")
        fi
    done

    # If no bullets found, use the raw description lines as fallback
    if [ ${#bullets[@]} -eq 0 ]; then
        for line in "${desc_lines[@]}"; do
            if [ -n "$line" ]; then
                bullets+=("- $line")
            fi
        done
    fi

    # Assign bullets to the highest bump group
    case "$highest_bump" in
        major) major_entries=("${bullets[@]}") ;;
        minor) minor_entries=("${bullets[@]}") ;;
        patch) patch_entries=("${bullets[@]}") ;;
    esac
done

# If no entries were parsed, fall back to using file basename
if [ ${#minor_entries[@]} -eq 0 ] && [ ${#patch_entries[@]} -eq 0 ] && [ ${#major_entries[@]} -eq 0 ]; then
    echo "Warning: No entries parsed from changesets. Using filenames as fallback."
    for f in "${files[@]}"; do
        basename=$(basename "$f" .md)
        patch_entries+=("- $basename")
    done
fi

# Generate version and date
VERSION="0.1.0"
DATE=$(date +%Y-%m-%d)

# Build the new changelog section
new_section=$(mktemp)
echo "## ${VERSION} — ${DATE}" >> "$new_section"
echo "" >> "$new_section"

if [ ${#major_entries[@]} -gt 0 ]; then
    echo "### Major Changes" >> "$new_section"
    echo "" >> "$new_section"
    for entry in "${major_entries[@]}"; do
        echo "$entry" >> "$new_section"
    done
    echo "" >> "$new_section"
fi

if [ ${#minor_entries[@]} -gt 0 ]; then
    echo "### Minor Changes" >> "$new_section"
    echo "" >> "$new_section"
    for entry in "${minor_entries[@]}"; do
        echo "$entry" >> "$new_section"
    done
    echo "" >> "$new_section"
fi

if [ ${#patch_entries[@]} -gt 0 ]; then
    echo "### Patch Changes" >> "$new_section"
    echo "" >> "$new_section"
    for entry in "${patch_entries[@]}"; do
        echo "$entry" >> "$new_section"
    done
    echo "" >> "$new_section"
fi

# Prepend to CHANGELOG.md
if [ -f "$CHANGELOG" ]; then
    # Insert after the header (first 3 lines typically)
    temp=$(mktemp)
    header_lines=3
    head -n $header_lines "$CHANGELOG" > "$temp"
    echo "" >> "$temp"
    cat "$new_section" >> "$temp"
    # Append the rest (skip the original header and any empty line after)
    tail -n +$((header_lines + 1)) "$CHANGELOG" 2>/dev/null | sed '/^$/d' >> "$temp" || true
    mv "$temp" "$CHANGELOG"
else
    # Scaffold new CHANGELOG
    {
        echo "# Changelog"
        echo ""
        echo "All notable changes to this project will be documented in this file."
        echo ""
        cat "$new_section"
    } > "$CHANGELOG"
fi

rm "$new_section"

# Remove consumed files
for f in "${files[@]}"; do
    rm "$f"
    echo "  Removed: $f"
done

echo "Done. CHANGELOG.md updated."
