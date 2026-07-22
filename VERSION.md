# Versioning Workflow

Versioning happens at commit time. Every code change that gets committed
must be accompanied by a changeset that defines the version bump for each
affected crate.

## Pre-commit (before any commit)

```bash
cargo fmt --all
cargo clippy --workspace --all-targets --all-features
cargo build --workspace
cargo test --workspace
```

## Commit Flow (versioning = committing)

For every code change:

1. Create `.changesets/<name>.md` with the bump for each affected crate
2. Run `bash scripts/consume-changesets.sh` (updates CHANGELOG.md, deletes changeset)
3. `git add -A`
4. `git commit -m "type(scope): short message"`

## Changeset File

```markdown
---
"crate-name": patch|minor|major - {{date}}
---

- Short description of change
- Another change
```

The changeset defines which crates changed and at what level. Multiple
crates can be bumped at different levels in a single changeset:

```yaml
---
"browser-daemon": minor
"cdp-core": patch
"cli": patch
---
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

| Bump    | When                                     | Example Version |
| ------- | ---------------------------------------- | --------------- |
| `patch` | Bug fixes, refactoring, internal cleanup | 0.0.0 → 0.0.1  (YYYY-MM-DD) |
| `minor` | New features, public API additions       | 0.0.1 → 0.1.0  (YYYY-MM-DD) |
| `major` | Breaking changes                         | 0.1.0 → 1.0.0  (YYYY-MM-DD) |

## Version Strategy

Each crate in this monorepo is versioned independently. A changeset file
can bump multiple crates at different levels in a single commit.

The `consume-changesets.sh` script groups entries by the highest bump
across all crates in a changeset and generates the CHANGELOG section
accordingly. No crate version numbers are written to files — the changeset
metadata serves as the source of truth for release tooling.
