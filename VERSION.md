# Versioning Workflow

Each crate in this monorepo is versioned independently. When code changes in a crate,
the changeset defines the bump level, and the consume script updates BOTH:
- The crate's Cargo.toml `version` field
- The crate's CHANGELOG.md with a new entry

Different crates can have different versions at the same time.

## Pre-commit

```bash
cargo fmt --all
cargo clippy --workspace
cargo build --workspace
cargo test --workspace
```

The pre-commit hook enforces this automatically. Set `SKIP_CHECKS=1` to bypass.

## Commit Flow

For every code change:

1. Create a changeset:
   ```bash
   bash scripts/create-changeset.sh
   ```
   This prompts for description and which crates changed with their bump types.

2. Consume the changeset:
   ```bash
   bash scripts/consume-changesets.sh
   ```
   This does the following for each affected crate:
   - Reads the current version from Cargo.toml
   - Bumps it according to the changeset (patch/minor/major)
   - Writes the new version to Cargo.toml
   - Prepends a new section to the crate's CHANGELOG.md
   - Deletes the changeset file

3. Stage and commit:
   ```bash
   git add -A
   git commit -m "type(scope): short description"
   ```

## Changeset File Format

```markdown
---
"cdp": minor
"cli": patch
---

- Add persistent browser mode with tab lifecycle
```

The changeset defines:
- Which crates changed
- Bump level per crate (patch/minor/major)
- Description of changes

## Bump Types

| Bump    | When                                     | Version Change |
| ------- | ---------------------------------------- | -------------- |
| `patch` | Bug fixes, refactoring, internal cleanup | 0.1.0 → 0.1.1 |
| `minor` | New features, public API additions       | 0.1.0 → 0.2.0 |
| `major` | Breaking changes                         | 0.1.0 → 1.0.0 |

## Current Versions

| Crate | Version |
|-------|---------|
| cdp | 0.3.0 |
| cli (gthings) | 0.3.0 |
| common | 0.3.0 |
| extraction | 0.3.0 |
| search | 0.3.0 |
