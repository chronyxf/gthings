# Versioning Workflow

Each crate versioned independently. One crate per commit. Never batch.

## Pipeline (ONE CRATE AT A TIME)

```
 1. changeset   — create .changeset/<name>.md
 2. changelog   — knope release (reads changeset, bumps version, updates CHANGELOG.md)
 3. commit      — git add + git commit (one line)
 4. publish     — cargo publish -p <crate>
```

Repeat for each crate with changes. Publish order: common → extraction → cdp → search → gthings.

---

## Step-by-Step

### 0. Which crate?

Check `git diff --stat`. Each file path tells you which crate:
- `crates/common/` → gthings-common
- `crates/extraction/` → gthings-extraction  
- `crates/cdp/` → gthings-cdp
- `crates/search/` → gthings-search
- `crates/cli/` → gthings (the CLI binary crate)

### 1. Changeset

Create a file in `.changeset/`. Name: `<crate>-<description>.md`.

Content format (knope 0.23, plain markdown, NO YAML frontmatter):

```
gthings-cdp: patch

- Fix: description of the fix
- Feat: description of the feature
```

First line = `crate-name: bump-type`. Body = bullet list.

Bump types:
| Bump    | When                     |
|---------|--------------------------|
| `patch` | Bug fixes, refactoring   |
| `minor` | New features             |
| `major` | Breaking changes         |

### 2. Changelog (knope)

```bash
knope release
```

This bumps Cargo.toml, updates CHANGELOG.md, deletes the changeset file.

### 3. Commit — ONE LINE ONLY

```bash
git add crates/<crate>/ CHANGELOG.md Cargo.toml Cargo.lock .changeset/
git commit -m "type(crate): description"
```

Commit message types:
| Type  | When                |
|-------|---------------------|
| `fix` | Bug fixes           |
| `feat`| New features        |
| `refactor` | Code changes  |
| `chore`| Build/config        |

One line. No body. Example:
- `fix(cdp): add session_id filter to lifecycle event predicate`
- `feat(search): add CAPTCHA/Sorry page detection`

### 4. Publish

```bash
cargo publish -p gthings-cdp
```

ALWAYS in dependency order (bottom-up):

```bash
cargo publish -p gthings-common      # 1st
cargo publish -p gthings-extraction   # 2nd
cargo publish -p gthings-cdp          # 3rd
cargo publish -p gthings-search       # 4th
cargo publish -p gthings              # 5th (cli)
```

---

## Pre-publish Checklist

- [ ] `cargo test --workspace` — all tests pass
- [ ] `cargo clippy --workspace -D warnings` — zero warnings
- [ ] `cargo login` — valid crates.io token
- [ ] Published in dependency order (bottom-up)
- [ ] Each Cargo.toml has `description`, `license`, `repository`, `homepage`
- [ ] Internal `path` deps have matching `version` field

Note: root package `gthings-tests` has `publish = false` — never pushed.
