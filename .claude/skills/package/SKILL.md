---
name: package
description: Orchestrate the privacyclaw packaging workflow. Asks for branch, target environment (dev/prod/both), and version bump type (major/minor/patch), then ensures a clean tree, runs tests, bumps the version, invokes the appropriate packager agent(s), commits, tags, and uploads release artifacts.
argument-hint: (no arguments needed — skill asks interactively)
context: fork
---

# Orchestrator — privacyclaw:package

You are the **packaging orchestrator**. Your sole job is to run the packaging
workflow safely and in the correct order. You do not build anything yourself —
you delegate to `dev-packager` and/or `prod-packager` agents.

---

## Step 0 — Ask the user three questions

Before touching anything, ask all three questions in a single message:

```
a) Which git branch should the package be built from?
   (press Enter for current branch)

b) Target environment: dev, prod, or both?

c) Version bump: major, minor, or patch?
```

Wait for answers. Record:
- `BRANCH` — the branch to use (default: current branch)
- `TARGET` — `dev`, `prod`, or `both`
- `BUMP` — `major`, `minor`, or `patch`

---

## Step 1 — Switch to the target branch and pull

```bash
git fetch origin
git checkout <BRANCH>
git pull origin <BRANCH>
```

If `git checkout` fails (branch does not exist): abort and tell the user.
If `git pull` fails (e.g. local diverged): abort and ask the user to resolve manually.

---

## Step 2 — Check working tree cleanliness

```bash
git status --porcelain
```

If the output is **empty**: proceed to Step 3.

If the output is **non-empty** (uncommitted changes exist):

1. Show the user the full diff:
   ```bash
   git diff HEAD
   git status
   ```

2. Ask:
   > "There are uncommitted changes. Choose:
   > A) Commit them now (provide a commit message)
   > B) Abort packaging"

3. If A: ask for a commit message, then:
   ```bash
   git add -A
   git commit -m "<user message>"
   git push origin <BRANCH>
   ```
   Verify push succeeded before continuing.

4. If B: abort with message "Packaging aborted. Resolve uncommitted changes and retry."

**Never auto-commit without the user's explicit approval and message.**

---

## Step 3 — Run the test suite

```bash
cargo test 2>&1
```

If any test fails: **abort packaging**.

Report to the user:
> "Tests failed. Packaging will not proceed. Fix the failures and retry."

Include the failing test names from the output.

If all tests pass: proceed.

---

## Step 4 — Compute new version

Read the current version from `Cargo.toml`:

```bash
grep '^version' Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/'
```

Apply the bump:
- `patch` → increment the third component: `0.2.3` → `0.2.4`
- `minor` → increment second, reset third: `0.2.3` → `0.3.0`
- `major` → increment first, reset others: `0.2.3` → `1.0.0`

Record `VERSION` (the new version string, e.g. `0.3.0`).

**Do not write anything yet.** Version is applied in Step 5 only if we have
confirmed the user wants to proceed.

Confirm with the user:
> "Ready to package v<VERSION> for <TARGET>. Branch: <BRANCH>. Proceed?"

Wait for confirmation. If declined: abort.

---

## Step 5 — Apply version bump (local only — do not commit or push yet)

```bash
# Cargo.toml
sed -i '' "s/^version = \".*\"/version = \"${VERSION}\"/" Cargo.toml

# Homebrew tap files
make tap-update-version
```

Verify `Cargo.toml` now contains `version = "<VERSION>"`.

---

## Step 6 — Invoke packager agent(s)

Pass `VERSION` and the repo root path in the invocation message.

### If TARGET = `dev`

```text
Agent("dev-packager", "VERSION: <VERSION>\nREPO: <absolute path to repo root>")
```

Wait for the DEV ARTIFACT MANIFEST.

### If TARGET = `prod`

```text
Agent("prod-packager", "VERSION: <VERSION>\nREPO: <absolute path to repo root>")
```

Wait for the PROD ARTIFACT MANIFEST.

### If TARGET = `both`

Invoke both agents **in parallel** (two simultaneous Agent tool calls):

```text
Agent("dev-packager",  "VERSION: <VERSION>\nREPO: <path>")
Agent("prod-packager", "VERSION: <VERSION>\nREPO: <path>")
```

Wait for both manifests.

---

## Step 7 — Verify artifact manifests

For each manifest received:
- `Status` must be `complete`. If any manifest has `Status: failed`, **abort**:
  > "Packaging failed: <reason from manifest>. Version bump has NOT been committed."
  > "Revert all local changes with:"
  > ```
  > git checkout Cargo.toml homebrew-privacyclaw/Formula/privacyclaw.rb homebrew-privacyclaw/Casks/privacyclaw-app.rb
  > ```

- Verify each listed artifact file exists on disk:
  ```bash
  ls -lh <artifact path>
  ```
  If an expected file is missing: treat as failure — abort with details.

If all manifests are complete and artifacts verified: proceed.

---

## Step 8 — Commit and push the version bump

```bash
git add Cargo.toml Cargo.lock homebrew-privacyclaw/Formula/privacyclaw.rb homebrew-privacyclaw/Casks/privacyclaw-app.rb
git commit -m "chore: bump version to v${VERSION}"
git push origin <BRANCH>
```

Note: `cargo build` inside the packager agents updates `Cargo.lock` with the
new version — it must be committed alongside `Cargo.toml`.

---

## Step 9 — Update changelog, create and push git tag

### Update CHANGELOG.md

Before tagging, prepend a new entry to `CHANGELOG.md` (create the file if it does not exist):

```markdown
## v<VERSION> — <YYYY-MM-DD>

### Target: <TARGET>

<one-line summary of what changed — derive from the git log since the previous tag>

### Artifacts
<list artifact filenames from the manifest>
```

To get the git log since the previous tag:
```bash
git log $(git describe --tags --abbrev=0 2>/dev/null || git rev-list --max-parents=0 HEAD)..HEAD --oneline
```

Commit the changelog alongside the version bump **in the same commit as Step 8** — go back and amend Step 8's staged files to include `CHANGELOG.md`:

```bash
git add CHANGELOG.md
git commit --amend --no-edit
git push origin <BRANCH> --force-with-lease
```

### Create and push git tag

```bash
git tag "v${VERSION}"
git push origin "v${VERSION}"
```

If the tag already exists: abort with:
> "Tag v<VERSION> already exists on origin. Was this version already released?"

---

## Step 10 — Upload release artifacts (prod only)

Skip this step entirely if `TARGET = dev`.

If `TARGET = prod` or `TARGET = both`:

### Create GitHub Release

```bash
gh release create "v${VERSION}" \
  --title "v${VERSION}" \
  --notes "Release v${VERSION}" \
  --target <BRANCH>
```

If a release for this tag already exists: abort with a warning and ask the
user whether to delete it and recreate, or skip artifact upload.

### Upload artifacts

From the PROD ARTIFACT MANIFEST, upload all listed artifacts:

```bash
gh release upload "v${VERSION}" \
  "dist/privacyclaw-${VERSION}-aarch64-apple-darwin.tar.gz" \
  "dist/privacyclaw-${VERSION}-x86_64-apple-darwin.tar.gz" \
  "<PKG_FINAL>"
```

Verify each upload by checking the release page:
```bash
gh release view "v${VERSION}"
```

---

## Step 11 — Sync local to tag

```bash
git pull origin <BRANCH>
git fetch --tags
```

Confirm the local HEAD is at the new tag:
```bash
git describe --tags --exact-match HEAD
```

---

## Step 12 — Report to user

Summarise the completed packaging run:

```
Packaging complete: v<VERSION> (<TARGET>)

Branch:  <BRANCH>
Tag:     v<VERSION> (pushed to origin)

Artifacts:
  <list from manifests>

Warnings:
  <any signing/notarization warnings from manifests, or "none">

Next steps (if prod):
  - Verify: brew tap kurianoff/privacyclaw && brew install privacyclaw
  - Verify: install .pkg and run privacyclaw --version
```

---

## Orchestration rules

- **Never commit the version bump until all artifacts are verified.**
  If build fails after version bump, Cargo.toml must be reverted to avoid a
  dangling version with no corresponding artifacts.
- **Never auto-commit dirty working tree changes** without user approval and
  an explicit commit message.
- **Never skip the test gate.** Tests must pass before any artifact is built.
- **Tag only after push.** The version commit must be on origin before tagging.
- **GitHub Release only for prod.** Dev artifacts are local-only.
- If the user answers any interactive prompt with a cancellation, abort cleanly
  and report exactly what state was left (e.g. "version bump applied locally
  but not committed").
