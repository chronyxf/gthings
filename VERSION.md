# Versioning Workflow

Each crate versioned independently. One crate per commit. Never batch.

## Pre-flight (before touching any crate)

1. **Diff overview**: `git diff --stat` — read the full list. Do not skip this step.
2. **Map files to crates** using the crate-prefix table below.
3. **Blockers**: `cargo test --workspace`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, and `cargo fmt --all -- --check` MUST pass first. If any fails, stop. Do not proceed until fixed.

### Crate-prefix table

| Prefix              | Package name           |
|---------------------|------------------------|
| `crates/common/`    | `gthings-common`       |
| `crates/extraction/`| `gthings-extraction`   |
| `crates/cdp/`       | `gthings-cdp`          |
| `crates/search/`    | `gthings-search`       |
| `crates/cli/`       | `gthings`              |

### Dependency order (publish bottom-up, never skip)

```
1. gthings-common
2. gthings-extraction
3. gthings-cdp
4. gthings-search
5. gthings          (CLI binary crate)
```

---

## Per-Crate Steps (repeat in dependency order)

### Step 0 — Which crate?

```bash
git diff --stat -- crates/<crate>/
```

If output is empty, this crate has no changes. Skip it. Move to next crate in dependency order.

### Step 1 — Write change bullets from git diff (anti-hallucination gate)

```bash
git diff -- crates/<crate>/
```

Force: **Read the diff output. Do NOT summarize from memory.**

Write each changed file as a bullet:

```
- Type: description (file.rs:line)
```

Types:

| Type       | When                            |
|------------|---------------------------------|
| `fix`      | Bug fix                         |
| `feat`     | New feature                     |
| `refactor` | Code restructuring, no behavior change |
| `chore`    | Build config, CI, tooling, deps |

### Step 2 — Bump type

| Bump    | Allowed types        | Example                                     |
|---------|----------------------|---------------------------------------------|
| `patch` | fix, refactor, chore | Fix timeout race in browser command queue   |
| `minor` | any feat             | Add CAPTCHA/Sorry page detection            |
| `major` | breaking API change  | Rename `Browser::new()` to `Browser::launch`|

### Step 3 — Changeset

Create `.changeset/<crate>-<desc>.md` with YAML frontmatter.

Use the helper script (recommended):

```bash
bash scripts/create-changeset.sh <package> <bump> "<description>" <type>
# e.g.
bash scripts/create-changeset.sh gthings-cdp patch "add session_id filter to lifecycle event predicate" fix
```

Or create it manually. Format (knope 0.23):

```
---
gthings-cdp: patch
---

- fix: add session_id filter to lifecycle event predicate
```

Rules:
- First line after `---` must be `<package>: <bump>`
- Package names MUST be unquoted (e.g. `gthings-common: patch`, not `"gthings-common"`). Quoting the name makes knope treat it as a literal quoted string that won't match the `[packages]` key in `knope.toml`, so the changeset is ignored.
- Body bullets must match Step 1 exactly (paste them).
- File name: `<crate-shortname>-<kebab-description>.md` (e.g. `cdp-session-id-filter.md`).

Validate/list the pending changesets:

```bash
bash scripts/consume-changesets.sh
```

### Step 4 — knope release

```bash
knope release
```

- If it succeeds: Cargo.toml bumped, CHANGELOG.md updated, changeset deleted.
- If it fails: try once more. If it fails twice, fall back to manual:
  1. Edit `crates/<crate>/Cargo.toml` — bump `version` field.
  2. Edit `crates/<crate>/CHANGELOG.md` — add entry under `## [new-version]`.
  3. Delete the changeset file manually.

### Step 5 — Commit (one line only)

Use Conventional Commits with a crate-name scope. The scope is the crate's short name (e.g. `gthings-cdp`, `gthings-common`).

```bash
git add crates/<crate>/ CHANGELOG.md Cargo.toml Cargo.lock .changeset/
git commit -m "feat(gthings-cdp): add session_id filter to lifecycle event predicate"
```

Examples:
- `feat(gthings-cdp): ...`
- `fix(gthings-common): ...`
- `refactor(gthings-extraction): ...`
- `chore(gthings): ...`

- One line. No body. No trailing period.
- The commit MUST pass the pre-commit hook (fmt + clippy + build + test). If the hook blocks, fix the code (run `cargo fmt --all` and fix clippy), do NOT bypass it.
- Keep Cargo.lock committed — its changes are normal and expected.

### Step 6 — Verify

```bash
git status --short
cargo fmt --all -- --check
```

Must be clean and formatted. If not, investigate and fix before proceeding.

### Step 7 — Publish

```bash
cargo publish -p <package>
```

- Skip if no crates.io token configured (`cargo login` not set).
- Publish in dependency order (bottom-up per table above).

---

## Important Notes

- **Cargo.lock changes are normal**: `cargo publish` and `knope release` both update it. Always stage and commit Cargo.lock. Do not discard it.
- **Git tags**: knope creates tags. Verify they are ancestors of HEAD: `git merge-base --is-ancestor <tag> HEAD`. If a tag is detached, do not push.
- **fmt + clippy must pass**: `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets --all-features -- -D warnings` MUST pass before any commit. The pre-commit hook is the enforcement — do not bypass it.
- **Manual fallback**: If `knope release` fails twice, do not keep retrying. Switch to manual edits (Step 4 fallback). After manual edits, still commit, verify, and publish normally.
- **Root package `gthings-tests`** has `publish = false` — never touched.

---

## Per-Crate Checklist (copyable)

```
[ ] git diff --stat -- crates/<crate>/
[ ] Write bullet points from diff (read output, do not guess)
[ ] Bump type: patch | minor | major
[ ] Create .changeset/<crate>-<desc>.md with YAML frontmatter (or bash scripts/create-changeset.sh)
[ ] Validate changesets: bash scripts/consume-changesets.sh
[ ] knope release (or manual fallback after 2 failures)
[ ] git commit -m "type(crate): description"  (must pass pre-commit hook)
[ ] git status --short                         (must be clean)
[ ] cargo publish -p <package>                 (skip if no token)
```

Repeat for each changed crate in dependency order. Never batch crates or commits.
