# Implementation Log: update-homebrew-formula-sidecar

Feature: Homebrew Formula Updates for Sidecar and T3 Pipeline
Branch: feature/homebrew-formula
OpenSpec ID: update-homebrew-formula-sidecar

---

### Task 1.1: Sidecar --version flag
Status: complete
Branch: task/homebrew-formula-1.1
Done:
  - Inserted version flag block in `packaging/privacyclaw-slm-sidecar` between `import sys` and `_MISSING = []`
  - Block checks `sys.argv[1] in ("--version", "-V")`, prints version string, exits 0
  - Verified: `python3 packaging/privacyclaw-slm-sidecar --version` prints `privacyclaw-slm-sidecar 0.1.0`
Issues found:
  - none
Contrarian verdict: approved

---

### Task 2.1: Formula depends_on python@3.11
Status: complete
Branch: task/homebrew-formula-2.1
Done:
  - Added `depends_on "python@3.11"` on the line immediately after `depends_on "llama.cpp"` in `packaging/homebrew/privacyclaw.rb`
Issues found:
  - none
Contrarian verdict: approved

---

### Task 3.1: Research pip checksums
Status: complete
Branch: task/homebrew-formula-3.1 (research only, no committed changes)
Done:
  - Ran `pip download --no-deps --python-version 3.11 --only-binary :all: --platform macosx_11_0_universal2` for 14 pure-Python packages
  - Ran `pip download --no-deps --python-version 3.11 --only-binary :all:` for pydantic-core arm64 (macosx_11_0_arm64) and x86_64 (macosx_10_12_x86_64 — no macosx_11_0 wheel available; using next lowest tag per spec allowance)
  - Computed SHA-256 checksums via `shasum -a 256` for all 16 wheels
  - Extracted PyPI download URLs via pip verbose output
Issues found:
  - No macosx_11_0_x86_64 wheel for pydantic-core 2.45.0 — fell back to macosx_10_12_x86_64 as permitted by spec. Documented with inline comment in formula.
Contrarian verdict: approved

---

### Task 3.2: Formula pip resource blocks (non-pydantic-core)
Status: complete
Branch: task/homebrew-formula-3.2
Done:
  - Added 14 resource blocks to `packaging/homebrew/privacyclaw.rb` after `depends_on` lines and before `def install`:
    fastapi 0.135.3, uvicorn 0.42.0, httpx 0.28.1, pydantic 2.12.5, starlette 1.0.0,
    anyio 4.13.0, sniffio 1.3.1, httpcore 1.0.9, h11 0.16.0, certifi 2026.2.25,
    idna 3.11, click 8.3.1, annotated-types 0.7.0, typing-extensions 4.15.0
  - typing-extensions appears exactly once (deduplication rule satisfied)
  - All wheels: py3-none-any (pure Python), macosx_11_0_universal2 platform tag
Issues found:
  - none
Contrarian verdict: approved

---

### Task 3.3: Formula pydantic-core platform-specific blocks
Status: complete
Branch: task/homebrew-formula-3.3
Done:
  - Added `on_macos do / on_arm do / resource "pydantic-core" ... end / end / on_intel do / resource "pydantic-core" ... end / end / end` block
  - arm64: pydantic_core-2.45.0-cp311-cp311-macosx_11_0_arm64.whl
  - x86_64: pydantic_core-2.45.0-cp311-cp311-macosx_10_12_x86_64.whl (fallback documented with inline comment)
  - Both blocks reference cp311 wheels with correct SHA-256 checksums
Issues found:
  - none
Contrarian verdict: approved

---

### Task 4.1: Formula install block
Status: complete
Branch: task/homebrew-formula-4.1
Done:
  - Replaced single-line `def install` with expanded block:
    1. `bin.install "privacyclaw"` (Rust binary first)
    2. `virtualenv_install_with_resources using: "python@3.11"` (creates venv with all pip resources)
    3. `bin.install "privacyclaw-slm-sidecar"` (sidecar script)
    4. `inreplace bin/"privacyclaw-slm-sidecar", /\A#!.+\n/, "#!#{opt_libexec}/bin/python3\n"` (shebang rewrite)
  - Verified: `virtualenv_install_with_resources` precedes `bin.install "privacyclaw-slm-sidecar"`
  - Verified: `inreplace` regex uses `\A` anchor (start of file)
  - Verified: replacement references `opt_libexec` (not `venv.root`)
Issues found:
  - none
Contrarian verdict: approved

---

### Task 5.1: Formula caveats T3 section
Status: complete
Branch: task/homebrew-formula-5.1
Done:
  - Appended T3 PII pipeline section to the caveats heredoc after the uninstall instructions
  - Content matches spec exactly: model auto-download notice, `privacyclaw models install`, sidecar debug usage
  - Heredoc syntax valid Ruby (EOS delimiters match, no indentation errors)
Issues found:
  - none
Contrarian verdict: approved

---

### Task 6.1: Formula brew test sidecar assertion
Status: complete
Branch: task/homebrew-formula-6.1
Done:
  - Appended sidecar smoke test inside `test do ... end` block after ca-path assertion:
    `assert_match "privacyclaw-slm-sidecar 0.1.0", shell_output("#{bin}/privacyclaw-slm-sidecar --version")`
  - Task 1.1 (--version flag) was already merged; dependency satisfied
  - Test does not import pip deps (--version exits before dependency guard)
Issues found:
  - none
Contrarian verdict: approved
