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

### Step 3 — knope derives version + changelog from Conventional Commits

Version bumps and the CHANGELOG are derived by knope directly from the Conventional Commits commit message (with crate scope). There is no changeset file. The commit message IS the change record.

### Step 4 — knope release

```bash
knope release
```

- If it succeeds: Cargo.toml bumped, CHANGELOG.md updated.
- If it fails: try once more. If it fails twice, fall back to manual:
  1. Edit `crates/<crate>/Cargo.toml` — bump `version` field.
  2. Edit `crates/<crate>/CHANGELOG.md` — add entry under `## [new-version]`.

### Step 5 — Commit (one line only)

Use Conventional Commits with a crate-name scope. The scope is the crate's short name (e.g. `gthings-cdp`, `gthings-common`).

```bash
git add crates/<crate>/ CHANGELOG.md Cargo.toml Cargo.lock
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
[ ] knope release (or manual fallback after 2 failures)
[ ] git commit -m "type(crate): description"  (must pass pre-commit hook)
[ ] git status --short                         (must be clean)
[ ] cargo publish -p <package>                 (skip if no token)
```

Repeat for each changed crate in dependency order. Never batch crates or commits.

---

## Docker Image Build & Push

The daemon image (Dockerfile) ships `gthings serve` for Docker deployments.
The image version tracks the `gthings-serve` crate version.

### Prerequisites
- Docker daemon running
- Authenticated to the target registry (`docker login`)

### Build (release)
```bash
docker build -t <namespace>/gthings:<gthings-serve-version> .
```
- Build from the workspace root (the Dockerfile is at the repo root).
- The builder stage runs `cargo build --release --locked -p gthings`.
- Example: `docker build -t yourname/gthings:0.1.0 .`

### Tag
```bash
docker tag <namespace>/gthings:<gthings-serve-version> <namespace>/gthings:latest
```
- `latest` is optional and only pushed intentionally.

### Push
```bash
docker push <namespace>/gthings:<gthings-serve-version>
docker push <namespace>/gthings:latest        # only if latest was tagged
```

### Release integration
- Build and push the image whenever `gthings-serve` is released (after the crates.io publish of `gthings-serve`).
- The image contains the full `gthings` binary (serve + all CLI subcommands); the entrypoint runs the daemon.

### Version rule
- Image tag = the `gthings-serve` crate version at release time.
- If the daemon image is released independently of a crate publish, use the next unpublished `gthings-serve` version.
