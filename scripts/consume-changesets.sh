#!/bin/bash
# Consume changeset files and update per-crate CHANGELOG.md files
# Usage: ./scripts/consume-changesets.sh
set -euo pipefail

CHANGESETS_DIR=".changesets"

if [ ! -d "$CHANGESETS_DIR" ]; then
    echo "No .changesets/ directory found"
    exit 0
fi

files=("$CHANGESETS_DIR"/*.md)
if [ ${#files[@]} -eq 0 ] || [ ! -f "${files[0]}" ]; then
    echo "No changeset files found"
    exit 0
fi

echo "Consuming ${#files[@]} changeset(s)..."

for f in "${files[@]}"; do
    basename=$(basename "$f" .md)
    
    # Parse frontmatter
    in_frontmatter=0
    declare -a crates=()
    declare -a bumps=()
    declare -a descriptions=()
    
    while IFS= read -r line; do
        if [[ "$line" == "---" ]]; then
            in_frontmatter=$((in_frontmatter + 1))
            continue
        fi
        if [ $in_frontmatter -eq 1 ]; then
            if [[ "$line" =~ ^\"([^\"]+)\":[[:space:]]*(patch|minor|major)$ ]]; then
                crates+=("${BASH_REMATCH[1]}")
                bumps+=("${BASH_REMATCH[2]}")
            fi
        elif [ $in_frontmatter -ge 2 ]; then
            trimmed=$(echo "$line" | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')
            if [ -n "$trimmed" ]; then
                descriptions+=("$trimmed")
            fi
        fi
    done < "$f"
    
    # For each crate, update its CHANGELOG.md
    for i in "${!crates[@]}"; do
        crate="${crates[$i]}"
        bump="${bumps[$i]}"
        
        # Determine changelog path
        changelog=""
        case "$crate" in
            cdp|cli|common|extraction|search)
                changelog="crates/$crate/CHANGELOG.md"
                ;;
            *)
                echo "  Warning: Unknown crate '$crate', skipping"
                continue
                ;;
        esac
        
        echo "  Updating $changelog ($bump)"
        
        # Extract last version
        last_version="0.0.0"
        if [ -f "$changelog" ]; then
            last_version=$(grep -m 1 '^## \[*[0-9]' "$changelog" | sed 's/^## \[*\([0-9]*\.[0-9]*\.[0-9]*\)\]*.*/\1/' || echo "0.0.0")
        fi
        
        # Parse and bump
        IFS='.' read -r major minor patch <<< "$last_version"
        case "$bump" in
            major) major=$((major + 1)); minor=0; patch=0 ;;
            minor) minor=$((minor + 1)); patch=0 ;;
            patch) patch=$((patch + 1)) ;;
        esac
        new_version="${major}.${minor}.${patch}"
        date=$(date +%Y-%m-%d)
        
        # Build new section
        new_section=$(mktemp)
        echo "## ${new_version} — ${date}" > "$new_section"
        
        # Categorize descriptions
        has_feat=0; has_fix=0; has_change=0
        feat_lines=(); fix_lines=(); change_lines=()
        for desc in "${descriptions[@]}"; do
            lower=$(echo "$desc" | tr '[:upper:]' '[:lower:]')
            if echo "$lower" | grep -q "fix\|bug\|repair\|patch"; then
                has_fix=1; fix_lines+=("$desc")
            elif echo "$lower" | grep -q "feat\|add\|new\|feature\|implement"; then
                has_feat=1; feat_lines+=("$desc")
            else
                has_change=1; change_lines+=("$desc")
            fi
        done
        
        # Write categorized entries
        {
            echo ""
            if [ ${#feat_lines[@]} -gt 0 ]; then
                echo "### Features"
                echo ""
                for line in "${feat_lines[@]}"; do clean="${line#- }"; echo "- $clean"; done
                echo ""
            fi
            if [ ${#fix_lines[@]} -gt 0 ]; then
                echo "### Fixes"
                echo ""
                for line in "${fix_lines[@]}"; do clean="${line#- }"; echo "- $clean"; done
                echo ""
            fi
            if [ ${#change_lines[@]} -gt 0 ]; then
                echo "### Changed"
                echo ""
                for line in "${change_lines[@]}"; do clean="${line#- }"; echo "- $clean"; done
                echo ""
            fi
        } >> "$new_section"
        
        # Prepend to changelog
        if [ -f "$changelog" ]; then
            temp=$(mktemp)
            header_lines=3
            head -n $header_lines "$changelog" > "$temp"
            echo "" >> "$temp"
            cat "$new_section" >> "$temp"
            tail -n +$((header_lines + 1)) "$changelog" 2>/dev/null >> "$temp" || true
            mv "$temp" "$changelog"
        else
            {
                echo "# Changelog — ${crate}"
                echo ""
                cat "$new_section"
            } > "$changelog"
        fi
        
        rm "$new_section"
        
        # Also update the crate's Cargo.toml version
        cargo_toml="crates/$crate/Cargo.toml"
        if [ "$crate" = "cli" ]; then
            cargo_toml="crates/cli/Cargo.toml"
        fi
        if [ -f "$cargo_toml" ]; then
            if [[ "$OSTYPE" == "darwin"* ]]; then
                sed -i '' "s/^version = \".*\"/version = \"$new_version\"/" "$cargo_toml"
            else
                sed -i "s/^version = \".*\"/version = \"$new_version\"/" "$cargo_toml"
            fi
            echo "    Updated $cargo_toml → $new_version"
        fi
    done
    
    # Remove consumed file
    rm "$f"
    echo "  Removed: $f"
done

echo "Done. Per-crate CHANGELOG.md files updated."
