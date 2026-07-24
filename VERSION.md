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

Commit one crate at a time. For each crate with changes:

1. Create a single-crate changeset:
   ```bash
   bash scripts/create-changeset.sh
   ```
   This prompts for description and which crate changed with its bump type.

2. Consume the changeset to bump version and update CHANGELOG.md:
   ```bash
   bash scripts/consume-changesets.sh
   ```

3. Stage and commit that crate separately:
   ```bash
   git add crates/<crate>/ tests/
   git commit -m "type(crate): short description"
   ```

Repeat for the next changed crate. Do NOT batch all crates into one commit.

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

## Publish to crates.io

Publish crates in dependency order (bottom-up). Each crate must be published
before its dependents.

```bash
# 1. Publish foundation crates (no internal deps)
cargo publish -p common
cargo publish -p cdp

# 2. Publish extraction (depends on common)
cargo publish -p extraction

# 3. Publish search (depends on common, extraction, cdp)
cargo publish -p search

# 4. Publish CLI binary last (depends on all of the above)
cargo publish -p gthings
```

After publishing, users install with:

```bash
cargo install gthings
```

### Pre-publish checklist

- [ ] Run `cargo test --workspace` — all tests pass
- [ ] Run `cargo clippy --workspace -D warnings` — zero warnings
- [ ] Verify each crate's Cargo.toml has `description`, `license`, `repository`, `homepage`
- [ ] Verify internal `path` dependencies also have `version` field
- [ ] Confirm `cargo login` with valid crates.io token
- [ ] Check each crate for `publish = false` if it should not be published
- [ ] Publish in dependency order (bottom-up)

Note: The root package `gthings-tests` has `publish = false` — it is never pushed
to crates.io. Only the 5 workspace member crates are published.

