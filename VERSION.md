# Versioning Workflow

Each crate in this monorepo is versioned independently. When code changes in a crate,
the changeset defines the bump level, and the consume script updates BOTH:
- The crate's Cargo.toml `version` field
- The crate's CHANGELOG.md with a new entry

Different crates can have different versions at the same time.

## Pipeline

One crate at a time. Do not batch multiple crates into one commit.

```
 1. format/lint/build/test
 2. changeset
 3. changelog
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

```bash
bash scripts/create-changeset.sh
```

Prompts for description and which crate changed with its bump type.

### 3. Changelog

```bash
bash scripts/consume-changesets.sh
```

Bumps version in Cargo.toml and prepends new entry to CHANGELOG.md.

### 4. Commit

```bash
git add crates/<crate>/ tests/
git commit -m "type(crate): short description"
```

One line, conventional commit format. Repeat steps 1-4 for each crate with changes.

### 5. Publish

```bash
cargo publish -p common
cargo publish -p cdp
cargo publish -p extraction
cargo publish -p search
cargo publish -p gthings
```

Publish in dependency order (bottom-up). Each crate must be published before its dependents resolve on crates.io.

## Changeset File Format

```markdown
---
"cdp": patch
---

- Refactor: spawn_blocking for sync I/O in browser.rs
```

Frontmatter: crate name in quotes, bump type. Body: bullet list of changes.

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
- [ ] Publish in dependency order: common → cdp → extraction → search → gthings

Note: root package `gthings-tests` has `publish = false` — never pushed. Only 5 workspace members publish.
