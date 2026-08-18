# Versioning Workflow

Each crate versioned independently. One crate per commit. Never batch.

## Pre-flight (before touching any crate)

1. **Diff overview**: `git diff --stat` — read the full list. Do not skip this step.
2. **Map files to crates** using the crate-prefix table below.
3. **Blockers**: `cargo test --workspace`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, and `cargo fmt --all -- --check` MUST pass first. If any fails, stop. Do not proceed until fixed.
4. **CI parity**: the same three gates also run automatically on Linux via GitHub Actions (ci.yml `check` job) — this verifies cross-platform, it does not replace the local pre-flight.

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

### Step 7 — Release (automated)

Release = create and push the version tag; the CI/CD `release` job then publishes the crate to crates.io (and, for `gthings-serve` tags, builds and pushes the Docker image):

```bash
git tag <crate>/v<version>          # e.g. gthings-serve/v0.1.1
git push origin <crate>/v<version>
```

- The tag version MUST match the crate manifest version (CI validates it).
- Manual `cargo publish -p <package>` remains possible as a fallback, but the tag flow is the standard path.

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
[ ] git tag <crate>/v<version> && git push origin <crate>/v<version>   (CD publishes)
```

Repeat for each changed crate in dependency order. Never batch crates or commits.

---

## Docker Image Build & Push

The daemon image (Dockerfile) ships `gthings serve` for Docker deployments.
The image version tracks the `gthings-serve` crate version.

The CI/CD `release` job builds and pushes the image automatically when a `gthings-serve/v<version>` tag is pushed (image tag = the `gthings-serve` crate version).

### Local build (testing / fallback)
```bash
docker build -t <namespace>/gthings:<gthings-serve-version> .
```
- Build from the workspace root (the Dockerfile is at the repo root).
- The builder stage runs `cargo build --release --locked -p gthings`.
- Example: `docker build -t yourname/gthings:0.1.0 .`

### Version rule
- Image tag = the `gthings-serve` crate version at release time.
- If the daemon image is released independently of a crate publish, use the next unpublished `gthings-serve` version.

---

## CI/CD Automation (GitHub Actions)

| Trigger | What runs |
|---------|-----------|
| push to `main` / PR | `check` (Linux fmt + clippy -D warnings + build + test) and `package` (crates.io dry-run for all crates) |
| tag `<crate>/v<version>` | `check` + `package` + `release` (publish the tagged crate; on `gthings-serve/*` tags also build+push the Docker image) |

- Secrets live in the `gthings-prod` environment: `CRATES_IO_TOKEN`, `DOCKER_USERNAME`, `DOCKER_PASSWORD`.
- The tag glob is `**` (slash-containing tags like `gthings-serve/v0.1.1` are supported).
