.PHONY: build test check format lint changeset version changelog commit

# Build all crates
build:
	cargo build --workspace

# Run all tests
test:
	cargo test --workspace

# Run clippy (lint)
lint:
	cargo clippy --workspace --all-targets --all-features -- -D warnings

# Format code
format:
	cargo fmt --all

# Check formatting without changes
format-check:
	cargo fmt --all -- --check

# Full pre-commit checklist
check: format-check lint test build

# Create a new changeset entry
# Usage: make changeset
changeset:
	@echo "Create a new file in .changesets/ directory"
	@echo "File name: <crate-name>-<description>.md"
	@echo ""
	@echo "Format:"
	@echo '---'
	@echo '"crate-name": patch|minor|major'
	@echo '---'
	@echo ""
	@echo "description of change"
	@echo ""
	@echo "Example: .changesets/cli-trace-flag.md"
	@echo '---'
	@echo '"cli": minor'
	@echo '---'
	@echo ""
	@echo "Add --trace flag for agent telemetry JSONL output"
	@echo ""

# Consume changesets: bump versions + update CHANGELOG
# This deletes .changesets/*.md and updates CHANGELOG.md
version:
	@echo "Consuming changesets..."
	@echo "This script will:"
	@echo "  1. Read all .changesets/*.md files"
	@echo "  2. Update CHANGELOG.md with entries"
	@echo "  3. Delete consumed changeset files"
	@echo "  4. Update version in workspace Cargo.toml"
	@if ls .changesets/*.md 2>/dev/null > /dev/null; then \
		echo "Found changesets:"; \
		ls .changesets/*.md; \
		echo ""; \
		echo "Run: ./scripts/consume-changesets.sh to execute"; \
	else \
		echo "No changesets found in .changesets/"; \
	fi

# Full release workflow
release: check version
	@echo "Ready to commit. Review changes with git diff."

# Install skills to global agent directory
install-skills:
	@bash scripts/install-skills.sh
	@echo "Skills installed. AI agents can now load: skill gthings"

# Full setup: build + test + install skills
setup: build test install-skills
	@echo "Setup complete. Run: gthings browser start --port 9222"
