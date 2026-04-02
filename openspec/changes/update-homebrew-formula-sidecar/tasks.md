# Tasks: Update Homebrew Formula to Package Python SLM Sidecar

## 1. Sidecar Script: --version flag

- [x] 1.1 Open `packaging/privacyclaw-slm-sidecar`. Locate line 22 (`import sys`)
      and line 24 (start of the `_MISSING = []` dependency guard loop).
      Insert the version flag block between them (after `import sys`, before `_MISSING`):
      ```python
      # ── Version flag (must precede dependency guard) ──────────────────────────────
      if len(sys.argv) > 1 and sys.argv[1] in ("--version", "-V"):
          print("privacyclaw-slm-sidecar 0.1.0")
          sys.exit(0)
      ```
      Verify: running `python3 packaging/privacyclaw-slm-sidecar --version` prints
      `privacyclaw-slm-sidecar 0.1.0` and exits 0 without importing fastapi/uvicorn.

## 2. Formula: depends_on python@3.11

- [x] 2.1 In `packaging/homebrew/privacyclaw.rb`, add `depends_on "python@3.11"` on
      the line immediately after `depends_on "llama.cpp"`.
      Verify: formula parses without error (`brew audit --formula packaging/homebrew/privacyclaw.rb`
      or manual inspection).

## 3. Formula: pip resource blocks

- [x] 3.1 Research pinned versions for all required pip packages. Run `pip download`
      in a clean temporary directory against the following explicit package list
      and record each `.whl` or `.tar.gz` filename and SHA-256 checksum
      (`shasum -a 256`):

      Direct dependencies:
        fastapi, uvicorn, httpx, pydantic

      Required transitive dependencies (verified via `pip show` after install):
        starlette (fastapi dep), anyio (starlette dep), sniffio (anyio dep),
        httpcore (httpx dep), h11 (httpcore dep), certifi (httpx dep),
        idna (httpx dep), click (uvicorn dep), annotated-types (pydantic dep),
        pydantic-core (pydantic dep), typing-extensions

      Deduplication rule: `typing-extensions` (and any other shared transitive dep)
      appears exactly ONCE as a resource block. If fastapi and pydantic require
      different minimum versions of `typing-extensions`, use the highest compatible
      pinned version that satisfies both.

      For pydantic-core: download the macosx_11_0_arm64 wheel AND the
      macosx_11_0_x86_64 wheel separately (do not use the sdist — it requires rustc).
      If no macosx_11_0 wheel exists, use the next lowest available macOS tag and
      document the tag used in a comment in the formula.

      Command pattern for non-pydantic-core packages:
        mkdir /tmp/pip-dl && cd /tmp/pip-dl
        pip download --no-deps --python-version 3.11 --only-binary :all: \
          --platform macosx_11_0_universal2 fastapi uvicorn httpx pydantic \
          starlette anyio sniffio httpcore h11 certifi idna click \
          annotated-types typing-extensions

- [x] 3.2 Add `resource` blocks for all packages except pydantic-core to
      `packaging/homebrew/privacyclaw.rb`, one per package, with pinned `url`
      and `sha256`. Place them after the `depends_on` lines and before `def install`.
      Format:
      ```ruby
      resource "fastapi" do
        url "https://files.pythonhosted.org/packages/.../fastapi-X.Y.Z-py3-none-any.whl"
        sha256 "..."
      end
      ```

- [x] 3.3 Add the pydantic-core resource blocks using platform-specific guards:
      ```ruby
      on_macos do
        on_arm do
          resource "pydantic-core" do
            url "https://files.pythonhosted.org/packages/.../pydantic_core-X.Y.Z-cp311-cp311-macosx_11_0_arm64.whl"
            sha256 "..."
          end
        end
        on_intel do
          resource "pydantic-core" do
            url "https://files.pythonhosted.org/packages/.../pydantic_core-X.Y.Z-cp311-cp311-macosx_11_0_x86_64.whl"
            sha256 "..."
          end
        end
      end
      ```
      Verify: both blocks reference `macosx_11_0` (or the documented fallback tag).

## 4. Formula: install block

- [x] 4.1 Replace the current `def install` block in `packaging/homebrew/privacyclaw.rb`
      with the following (preserving `bin.install "privacyclaw"` as the first step):
      ```ruby
      def install
        bin.install "privacyclaw"
        virtualenv_install_with_resources using: "python@3.11"
        bin.install "privacyclaw-slm-sidecar"
        inreplace bin/"privacyclaw-slm-sidecar",
                  /\A#!.+\n/,
                  "#!#{opt_libexec}/bin/python3\n"
      end
      ```
      Verify:
      - `virtualenv_install_with_resources` appears before `bin.install "privacyclaw-slm-sidecar"`
      - The `inreplace` regex uses `\A` (start of file) not `^` (start of line)
      - The replacement references `opt_libexec` (not `venv.root` or any versioned prefix)

## 5. Formula: caveats

- [x] 5.1 Expand the `def caveats` heredoc in `packaging/homebrew/privacyclaw.rb`
      to append a T3 model setup section after the existing content:
      ```
      T3 PII pipeline (SLM sidecar):

        On first run with T3 enabled, privacyclaw auto-downloads the smollm2-135m
        model (~135 MB). To manage models manually:

          privacyclaw models install <model-name>

        The Python sidecar is available for advanced or debugging use:

          privacyclaw-slm-sidecar --version
          SIDECAR_PORT=16442 privacyclaw-slm-sidecar
      ```
      Verify: caveats heredoc is syntactically valid Ruby (no unmatched EOS or
      indentation errors).

## 6. Formula: test block

- [x] 6.1 Append a sidecar smoke test to the existing `test do` block in
      `packaging/homebrew/privacyclaw.rb`, after the existing ca-path assertion:
      ```ruby
      # Verify sidecar script is installed and self-identifies.
      assert_match "privacyclaw-slm-sidecar 0.1.0",
                   shell_output("#{bin}/privacyclaw-slm-sidecar --version")
      ```
      Verify: the new assertion is inside `test do ... end` (not after `end`).
      Confirm the test does not import any pip dependency (the --version flag
      exits before the dependency guard runs).
