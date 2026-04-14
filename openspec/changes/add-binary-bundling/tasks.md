# Tasks: Add Binary Bundling for llama-server and Sidecar

## Overview

All changes are in build infrastructure (Makefile, formula files, CI workflow,
postinstall script). No Rust source changes. Tasks are ordered by dependency:
Makefile foundation → formula update → tap sync → CI → postinstall → tests.

Tasks T1–T5 are on the critical path and must be done in order.
T6 (tap sync) depends on T4 (formula update) and T5 (tap-sync-formula target).
T7 (CI) can be written in parallel with T6 but requires T2 and T5 to exist.
T8 (postinstall venv) is independent of T1–T7.
T9–T10 (tests) depend on all prior tasks being done.
T11 (documentation) is independent.

---

## Tasks

- [x] **T1 — Makefile: add LLAMA_CPP_TAG variable and print-llama-tag target**

  Add to `Makefile` (near the top, after existing variable declarations):

  ```makefile
  # Pinned llama.cpp release to bundle with the tarball.
  # To update: change this tag, then run: make tarball
  LLAMA_CPP_TAG ?= b5000

  # llama.cpp GitHub release asset base URL
  LLAMA_RELEASE_BASE := https://github.com/ggerganov/llama.cpp/releases/download/$(LLAMA_CPP_TAG)
  ```

  Add `print-llama-tag` to the `.PHONY` declaration and add the target:

  ```makefile
  print-llama-tag:
      @echo $(LLAMA_CPP_TAG)
  ```

  Verify: `make -s print-llama-tag` prints `b5000` (or the pinned value) and
  nothing else. `grep 'LLAMA_CPP_TAG' Makefile` finds exactly one definition.

---

- [x] **T2 — Makefile: add tarball target**

  Add to `Makefile` after the `release` target:

  ```makefile
  TARBALL := /tmp/privacyclaw-$(VERSION)-universal-apple-darwin.tar.gz

  .PHONY: tarball
  tarball: release
      mkdir -p $(DIST)/llama-tmp
      curl -fL "$(LLAMA_RELEASE_BASE)/llama-$(LLAMA_CPP_TAG)-bin-macos-arm64.zip" \
          -o $(DIST)/llama-tmp/llama-arm64.zip
      unzip -o $(DIST)/llama-tmp/llama-arm64.zip -d $(DIST)/llama-tmp/arm64/
      curl -fL "$(LLAMA_RELEASE_BASE)/llama-$(LLAMA_CPP_TAG)-bin-macos-x86_64.zip" \
          -o $(DIST)/llama-tmp/llama-x86.zip
      unzip -o $(DIST)/llama-tmp/llama-x86.zip -d $(DIST)/llama-tmp/x86_64/
      @# Verify exactly one llama-server extracted per arch
      @ARM_COUNT=$$(find $(DIST)/llama-tmp/arm64 -name 'llama-server' -type f | wc -l | tr -d ' '); \
       X86_COUNT=$$(find $(DIST)/llama-tmp/x86_64 -name 'llama-server' -type f | wc -l | tr -d ' '); \
       [ "$$ARM_COUNT" = "1" ] || (echo "ERROR: expected 1 llama-server in arm64 zip, found $$ARM_COUNT"; exit 1); \
       [ "$$X86_COUNT" = "1" ] || (echo "ERROR: expected 1 llama-server in x86_64 zip, found $$X86_COUNT"; exit 1)
      lipo -create \
          $$(find $(DIST)/llama-tmp/arm64 -name 'llama-server' -type f) \
          $$(find $(DIST)/llama-tmp/x86_64 -name 'llama-server' -type f) \
          -output $(DIST)/llama-server
      @# Verify universal binary
      @file $(DIST)/llama-server | grep -q 'universal binary' || \
          (echo "ERROR: llama-server is not a universal binary"; exit 1)
      chmod +x $(DIST)/llama-server
      cp packaging/privacyclaw-slm-sidecar $(DIST)/privacyclaw-slm-sidecar
      chmod +x $(DIST)/privacyclaw-slm-sidecar
      tar -czf $(TARBALL) -C $(DIST) privacyclaw llama-server privacyclaw-slm-sidecar
      @echo "Tarball: $(TARBALL)"
      @echo "SHA256:  $$(shasum -a 256 $(TARBALL) | awk '{print $$1}')"
      @echo ""
      @echo "Next steps:"
      @echo "  1. Upload $(TARBALL) to GitHub Release v$(VERSION)"
      @echo "  2. Run: make tap-sync-formula SHA256=<above>"
  ```

  Verify: `make tarball 2>&1 | grep -E 'Tarball:|ERROR:'` — on a machine with
  network access this should print `Tarball: /tmp/privacyclaw-…tar.gz` with no
  `ERROR:` lines. On CI, the integration test in T7 covers this end-to-end.
  Unit-verifiable now: `grep 'tarball:' Makefile` finds the target; `grep
  'find.*llama-server' Makefile` finds the count-check lines.

---

- [x] **T3 — Makefile: update _pkg-layout to copy sidecar to SHARE_DIR**

  In the `_pkg-layout` target, add after the existing `llama-server` copy block:

  ```makefile
      @if [ -f "packaging/privacyclaw-slm-sidecar" ]; then \
          cp packaging/privacyclaw-slm-sidecar $(SHARE_DIR)/privacyclaw-slm-sidecar; \
          chmod +x $(SHARE_DIR)/privacyclaw-slm-sidecar; \
          echo "Bundled privacyclaw-slm-sidecar"; \
      else \
          echo "WARN: privacyclaw-slm-sidecar not found — Tier 3 /replace endpoint unavailable"; \
      fi
  ```

  Verify: `grep -A5 'privacyclaw-slm-sidecar' Makefile` shows the copy block
  inside `_pkg-layout`. `tests/pkg_build_test.rs::dist_dir_referenced_in_makefile`
  continues to pass (no assert on sidecar presence yet; T10 adds that).

---

- [x] **T4 — packaging/homebrew/privacyclaw.rb: remove llama.cpp dependency and add llama-server install**

  In `packaging/homebrew/privacyclaw.rb`:

  1. Remove the line: `depends_on "llama.cpp"`

  2. Update the `on_macos` block to use the universal tarball URL:
     ```ruby
     on_macos do
       url "https://github.com/kurianoff/kladovka/releases/download/v#{version}/privacyclaw-#{version}-universal-apple-darwin.tar.gz"
       sha256 "PLACEHOLDER_SHA256_REPLACE_BEFORE_PUBLISHING"
     end
     ```
     (Remove the per-arch `if Hardware::CPU.arm?` branching; the single URL and
     SHA-256 apply to both architectures.)

  3. In the `install` block, add `bin.install "llama-server"` immediately after
     `bin.install "privacyclaw"`:
     ```ruby
     def install
       bin.install "privacyclaw"
       bin.install "llama-server"
       virtualenv_install_with_resources using: "python@3.11"
       bin.install "privacyclaw-slm-sidecar"
       inreplace bin/"privacyclaw-slm-sidecar",
                 /\A#!.+\n/,
                 "#!#{opt_libexec}/bin/python3\n"
     end
     ```

  4. Add a header comment to the file (after the `# typed: false` line):
     ```ruby
     # SOURCE OF TRUTH: This file is the authoritative formula.
     # The tap formula at homebrew-privacyclaw/Formula/privacyclaw.rb is
     # GENERATED from this file via: make tap-sync-formula SHA256=<hash>
     # Do not edit the tap formula directly.
     ```

  Verify: `grep 'depends_on "llama.cpp"' packaging/homebrew/privacyclaw.rb`
  returns empty. `grep 'bin.install "llama-server"' packaging/homebrew/privacyclaw.rb`
  returns a match. `grep 'universal-apple-darwin' packaging/homebrew/privacyclaw.rb`
  returns a match.

---

- [x] **T5 — Makefile: add tap-sync-formula target and deprecate brew-package**

  1. Add `tap-sync-formula` to the `.PHONY` declaration.

  2. Add the `tap-sync-formula` target after `tap-update-version`:

     ```makefile
     # Sync the tap formula from the source formula.
     # Run after publishing a release tarball:
     #   make tap-sync-formula SHA256=<sha256-of-tarball>
     tap-sync-formula:
         @[ -n "$(SHA256)" ] || (echo "Usage: make tap-sync-formula SHA256=<hash>"; exit 1)
         cp packaging/homebrew/privacyclaw.rb homebrew-privacyclaw/Formula/privacyclaw.rb
         sed -i '' \
             -e 's|PLACEHOLDER_SHA256_REPLACE_BEFORE_PUBLISHING|$(SHA256)|g' \
             -e 's|privacyclaw-.*-universal-apple-darwin\.tar\.gz|privacyclaw-$(VERSION)-universal-apple-darwin.tar.gz|g' \
             homebrew-privacyclaw/Formula/privacyclaw.rb
         @echo "Tap formula updated. Review and commit homebrew-privacyclaw/Formula/privacyclaw.rb"
     ```

  3. Add a deprecation comment to `brew-package` and make it print a notice:

     ```makefile
     # DEPRECATED: use `make tarball` instead.
     # brew-package produces a single-arch arm64-only tarball of privacyclaw only.
     # It is retained for backward compatibility and will be removed in a future
     # major version.
     brew-package:
         @echo "DEPRECATION NOTICE: brew-package is deprecated. Use 'make tarball' instead."
         cargo build --release --target aarch64-apple-darwin
         ...existing recipe unchanged...
     ```

  Verify: `make tap-sync-formula` (without SHA256) prints usage error and exits 1.
  `make tap-sync-formula SHA256=testhash` copies the formula and substitutes the
  hash: `grep 'testhash' homebrew-privacyclaw/Formula/privacyclaw.rb` returns a
  match. `make brew-package` prints the deprecation notice before building.

---

- [x] **T6 — Sync tap formula: run tap-sync-formula with placeholder hash and update version**

  This task brings the tap formula to parity with the source formula.
  Since no real release tarball exists yet (that happens in CI), use a placeholder:

  ```bash
  cd /path/to/worktree
  make tap-sync-formula SHA256=PLACEHOLDER_SHA256_REPLACE_BEFORE_PUBLISHING
  ```

  Then manually update the `version` field in
  `homebrew-privacyclaw/Formula/privacyclaw.rb` to match `Cargo.toml` version.

  The tap formula must contain (after this task):
  - `class Privacyclaw < Formula`
  - `def install` block with `bin.install "llama-server"`
  - `depends_on "python@3.11"` (from source formula)
  - No `depends_on "llama.cpp"`
  - `service do` block
  - `test do` block with sidecar `--version` assertion

  Verify: `cargo test --test brew_formula_test` passes. `grep 'depends_on
  "llama.cpp"' homebrew-privacyclaw/Formula/privacyclaw.rb` returns empty.
  `grep 'bin.install "llama-server"' homebrew-privacyclaw/Formula/privacyclaw.rb`
  returns a match.

---

- [x] **T7 — Create .github/workflows/release.yml**

  Create `.github/workflows/release.yml` implementing the full release pipeline:

  ```yaml
  name: Release

  on:
    push:
      tags:
        - 'v*.*.*'

  jobs:
    release:
      runs-on: macos-14
      permissions:
        contents: write

      steps:
        - name: Checkout
          uses: actions/checkout@v4
          with:
            fetch-depth: 1

        - name: Install Rust targets
          run: |
            rustup target add aarch64-apple-darwin x86_64-apple-darwin

        - name: Cache Rust artifacts
          uses: actions/cache@v4
          with:
            path: |
              ~/.cargo/registry
              ~/.cargo/git
              target
            key: ${{ runner.os }}-cargo-release-${{ hashFiles('**/Cargo.lock') }}
            restore-keys: |
              ${{ runner.os }}-cargo-release-

        - name: Set VERSION
          run: echo "VERSION=${GITHUB_REF_NAME#v}" >> $GITHUB_ENV

        - name: Build universal privacyclaw binary
          run: make release

        - name: Read LLAMA_CPP_TAG from Makefile
          run: echo "LLAMA_CPP_TAG=$(make -s print-llama-tag)" >> $GITHUB_ENV

        - name: Download llama-server binaries
          run: |
            BASE="https://github.com/ggerganov/llama.cpp/releases/download/${{ env.LLAMA_CPP_TAG }}"
            mkdir -p dist/llama-tmp
            curl -fL "$BASE/llama-${{ env.LLAMA_CPP_TAG }}-bin-macos-arm64.zip" \
                -o dist/llama-tmp/llama-arm64.zip
            curl -fL "$BASE/llama-${{ env.LLAMA_CPP_TAG }}-bin-macos-x86_64.zip" \
                -o dist/llama-tmp/llama-x86.zip
            unzip -o dist/llama-tmp/llama-arm64.zip -d dist/llama-tmp/arm64/
            unzip -o dist/llama-tmp/llama-x86.zip  -d dist/llama-tmp/x86_64/
            ARM_LLAMA=$(find dist/llama-tmp/arm64  -name 'llama-server' -type f)
            X86_LLAMA=$(find dist/llama-tmp/x86_64 -name 'llama-server' -type f)
            [ -n "$ARM_LLAMA" ] || (echo "ERROR: llama-server not found in arm64 zip"; exit 1)
            [ -n "$X86_LLAMA" ] || (echo "ERROR: llama-server not found in x86_64 zip"; exit 1)
            lipo -create "$ARM_LLAMA" "$X86_LLAMA" -output dist/llama-server
            file dist/llama-server | grep -q 'universal binary' || \
                (echo "ERROR: llama-server is not a universal binary"; exit 1)
            chmod +x dist/llama-server

        - name: Copy sidecar
          run: |
            cp packaging/privacyclaw-slm-sidecar dist/privacyclaw-slm-sidecar
            chmod +x dist/privacyclaw-slm-sidecar

        - name: Assemble tarball
          run: |
            TARBALL="dist/privacyclaw-${{ env.VERSION }}-universal-apple-darwin.tar.gz"
            tar -czf "$TARBALL" -C dist privacyclaw llama-server privacyclaw-slm-sidecar
            echo "TARBALL=$TARBALL" >> $GITHUB_ENV
            SHA256=$(shasum -a 256 "$TARBALL" | awk '{print $1}')
            echo "SHA256=$SHA256" >> $GITHUB_ENV
            echo "SHA-256: $SHA256"

        - name: Create GitHub Release and upload tarball
          env:
            GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
          run: |
            gh release create "${{ github.ref_name }}" \
              --title "Privacyclaw ${{ github.ref_name }}" \
              --generate-notes \
              "${{ env.TARBALL }}"

        - name: Sync tap formula
          run: make tap-sync-formula SHA256=${{ env.SHA256 }}

        - name: Commit updated tap formula
          run: |
            git config user.name  "github-actions[bot]"
            git config user.email "github-actions[bot]@users.noreply.github.com"
            git add homebrew-privacyclaw/Formula/privacyclaw.rb
            git diff --cached --quiet || \
              git commit -m "chore(tap): sync formula for ${{ github.ref_name }}"
            git push origin HEAD:main
  ```

  Verify: File is valid YAML (`python3 -c "import yaml; yaml.safe_load(open('.github/workflows/release.yml'))"` exits 0). File contains `macos-14` runner, `print-llama-tag` call, `tap-sync-formula` step.

---

- [x] **T8 — packaging/postinstall: add Python virtualenv creation after sidecar copy**

  After the sidecar copy block (lines ~68–71), add a venv creation section.
  The new block runs as the actual user (not root) and is best-effort:

  ```bash
  # ── Python virtualenv for privacyclaw-slm-sidecar ─────────────────────────────
  VENV_DIR="$USER_HOME/Library/Application Support/privacyclaw/sidecar-venv"
  SIDECAR_REQUIREMENTS="fastapi uvicorn httpx pydantic"

  if [ -f "$SLM_SIDECAR_DEST" ]; then
      echo "[privacyclaw] Creating Python virtualenv for sidecar..."
      if su -m "$ACTUAL_USER" -c "python3 -m venv \"$VENV_DIR\"" 2>/dev/null; then
          if su -m "$ACTUAL_USER" -c "\"$VENV_DIR/bin/pip\" install --quiet $SIDECAR_REQUIREMENTS" 2>/dev/null; then
              echo "[privacyclaw] Sidecar virtualenv ready at $VENV_DIR"
          else
              echo "[privacyclaw] WARNING: pip install failed — sidecar may not work. Run manually:"
              echo "[privacyclaw]   python3 -m venv \"$VENV_DIR\" && \"$VENV_DIR/bin/pip\" install $SIDECAR_REQUIREMENTS"
          fi
      else
          echo "[privacyclaw] WARNING: python3 -m venv failed — sidecar may not work."
          echo "[privacyclaw]   Ensure Python 3.9+ is installed and try: python3 -m venv \"$VENV_DIR\""
      fi
  fi
  ```

  Verify: `grep 'venv' packaging/postinstall` returns lines containing the venv
  block. `grep 'WARNING.*pip install failed' packaging/postinstall` returns a
  match. `bash -n packaging/postinstall` exits 0 (syntax check).

---

- [x] **T9 — tests/brew_formula_test.rs: assert llama-server presence and no llama.cpp dependency**

  Add two new test functions to `tests/brew_formula_test.rs`:

  ```rust
  /// Tap formula must not have depends_on "llama.cpp" (bundled directly now).
  #[test]
  fn tap_formula_has_no_llama_cpp_dependency() {
      let formula = tap_root().join("Formula/privacyclaw.rb");
      let content = std::fs::read_to_string(&formula).unwrap();
      assert!(
          !content.contains(r#"depends_on "llama.cpp""#),
          "tap formula must not declare depends_on \"llama.cpp\""
      );
  }

  /// Tap formula must install llama-server directly from the tarball.
  #[test]
  fn tap_formula_installs_llama_server() {
      let formula = tap_root().join("Formula/privacyclaw.rb");
      let content = std::fs::read_to_string(&formula).unwrap();
      assert!(
          content.contains(r#"bin.install "llama-server""#),
          "tap formula must include bin.install \"llama-server\""
      );
  }
  ```

  Also add an assertion to the existing `formula_privacyclaw_rb_exists_and_valid`
  test to check that the source formula also lacks the `depends_on "llama.cpp"` line:

  ```rust
  // T4: source formula must not depend on llama.cpp
  let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
      .join("packaging/homebrew/privacyclaw.rb");
  let source_content = std::fs::read_to_string(&source).unwrap();
  assert!(
      !source_content.contains(r#"depends_on "llama.cpp""#),
      "source formula must not declare depends_on \"llama.cpp\""
  );
  ```

  Verify: `cargo test --test brew_formula_test` passes with all four test
  functions (two existing + two new).

---

- [x] **T10 — tests/pkg_build_test.rs: assert sidecar copy step in Makefile**

  Add a new test function to `tests/pkg_build_test.rs`:

  ```rust
  #[test]
  fn makefile_pkg_layout_copies_sidecar() {
      let makefile = project_root().join("Makefile");
      let content = std::fs::read_to_string(&makefile).unwrap();
      assert!(
          content.contains("privacyclaw-slm-sidecar"),
          "Makefile _pkg-layout must copy privacyclaw-slm-sidecar to SHARE_DIR"
      );
  }

  #[test]
  fn makefile_has_tarball_target() {
      let makefile = project_root().join("Makefile");
      let content = std::fs::read_to_string(&makefile).unwrap();
      assert!(content.contains("tarball:"), "Makefile must have tarball: target");
      assert!(content.contains("LLAMA_CPP_TAG"), "Makefile must declare LLAMA_CPP_TAG");
      assert!(content.contains("print-llama-tag"), "Makefile must have print-llama-tag target");
      assert!(content.contains("tap-sync-formula"), "Makefile must have tap-sync-formula target");
  }
  ```

  Verify: `cargo test --test pkg_build_test` passes with all existing tests plus
  the two new test functions.

---

- [x] **T11 — Makefile: add documentation comment to tap-update-version target**

  Update the comment above `tap-update-version` to clarify the new workflow:

  ```makefile
  # Update version in tap cask (run after bumping Cargo.toml version).
  # NOTE: The tap FORMULA is now fully regenerated by tap-sync-formula, not
  # patched by this target. tap-update-version still patches the cask.
  tap-update-version:
      sed -i '' 's/version "[0-9]*\.[0-9]*\.[0-9]*"/version "$(VERSION)"/' \
        homebrew-privacyclaw/Casks/privacyclaw-app.rb
  ```

  (Remove the `homebrew-privacyclaw/Formula/privacyclaw.rb` line from the sed
  command — formula sync now happens via `tap-sync-formula`, not a version patch.)

  Verify: `grep -A4 'tap-update-version:' Makefile` shows the sed command no
  longer references `Formula/privacyclaw.rb`.
