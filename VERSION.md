# Versioning Workflow

Each crate has its own CHANGELOG.md. Changes are tracked per-crate via changeset files.
A single commit can affect multiple crates with different bump levels.

## Pre-commit

```bash
cargo fmt --all
cargo clippy --workspace --all-targets --all-features
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
   This prompts for description and affected crates with bump types.

2. Consume changesets (updates per-crate CHANGELOG.md files):
   ```bash
   bash scripts/consume-changesets.sh
   ```

3. Stage and commit:
   ```bash
   git add -A
   git commit -m "type(scope): short description"
   ```
   Commit messages are single-line. Full descriptions live in changesets and changelogs.

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

| Bump    | When                                     |
| ------- | ---------------------------------------- |
| `patch` | Bug fixes, refactoring, internal cleanup |
| `minor` | New features, public API additions       |
| `major` | Breaking changes                         |

## Version Source of Truth

Each crate's version is derived from its CHANGELOG.md. The latest version entry
in each per-crate changelog is the source of truth for that crate.
There is no global version — crates version independently.
