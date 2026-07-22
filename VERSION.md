# Versioning Workflow

## Pre-commit

```bash
cargo fmt --all
cargo clippy --workspace --all-targets --all-features
cargo build --workspace
cargo test --workspace
```

## Changeset

Create `.changesets/<name>.md`:

```markdown
---
"crate-name": patch|minor|major
---

- Short description of change
- Another change
```

Then:
```bash
bash scripts/consume-changesets.sh   # updates CHANGELOG.md, deletes changeset
git add -A
git commit -m "type(scope): short message"
```

## Commit Types

| Type | Use |
|------|-----|
| `feat` | New feature |
| `fix` | Bug fix |
| `refactor` | Code restructure |
| `chore` | Config, tooling, version |
| `docs` | Documentation |
| `style` | Formatting |

## Bump Types

| Bump | When |
|------|------|
| `patch` | Bug fixes, refactoring |
| `minor` | New features |
| `major` | Breaking changes |
