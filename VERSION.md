# Versioning Workflow

Each crate in this monorepo is versioned independently. When code changes in a crate,
knope reads the changeset markdown files, bumps Cargo.toml versions, and updates CHANGELOG.md.

Different crates can have different versions at the same time.

## Pipeline

One crate at a time. Do not batch multiple crates into one commit.

```
 1. format/lint/build/test
 2. changeset
 3. changelog (knope)
 4. commit (1 line)
 5. publish
```

### 1. Format → Lint → Build → Test

Pre-commit hook enforces these automatically. Set `SKIP_CHECKS=1` to bypass.

```bash
cargo fmt --all
cargo clippy --workspace
cargo build --workspace
cargo test --workspace
```

### 2. Changeset

Create a changeset file in `.changesets/` (the configured changes directory for knope) with YAML frontmatter:

```markdown
---
"gthings-common": patch
---

- Refactor: spawn_blocking for sync I/O in browser.rs
```

All bump types: `patch`, `minor`, `major`.

### 3. Changelog (knope)

```bash
knope release
```

- Reads changeset files from `.changesets/`
- Bumps versions in Cargo.toml
- Updates dependency version constraints in dependent crates
- Prepends entries to each crate's CHANGELOG.md
- Deletes consumed changeset files

### 4. Commit

```bash
git add crates/<crate>/ tests/ .changesets/
git commit -m "type(crate): short description"
```

One line, conventional commit format. Repeat steps 1-4 for each crate with changes.

### 5. Publish

Publish in dependency order (bottom-up):

```bash
cargo publish -p gthings-common
cargo publish -p gthings-extraction
cargo publish -p gthings-cdp
cargo publish -p gthings-search
cargo publish -p gthings
```

## Changeset File Format

Standard knope changeset markdown:

```markdown
---
"crate-name": patch
---

- Description of the change

More details if needed.
```

Frontmatter: crate name in quotes, bump type. Body: bullet list or paragraph.

## Bump Types

| Bump    | When                                     | Version Change |
| ------- | ---------------------------------------- | -------------- |
| `patch` | Bug fixes, refactoring, internal cleanup | 0.1.0 → 0.1.1 |
| `minor` | New features, public API additions       | 0.1.0 → 0.2.0 |
| `major` | Breaking changes                         | 0.1.0 → 1.0.0 |

## Pre-publish Checklist

- [ ] `cargo test --workspace` — all tests pass
- [ ] `cargo clippy --workspace -D warnings` — zero warnings
- [ ] Each Cargo.toml has `description`, `license`, `repository`, `homepage`
- [ ] Internal `path` deps have matching `version` field
- [ ] `cargo login` with valid crates.io token
- [ ] Publish in dependency order (bottom-up)

Note: root package `gthings-tests` has `publish = false` — never pushed. Only 5 workspace members publish.
