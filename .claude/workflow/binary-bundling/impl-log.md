# Implementation Log: Add Binary Bundling for llama-server and Sidecar

Feature: Binary Bundling Strategy for Sidecar and llama-server
Branch: feature/binary-bundling
Worktree: /Users/constantine/github/kurianoff/worktrees/feature-binary-bundling

## Status: complete

All 11 tasks implemented, Contrarian approved, merged to feature branch.

---

### Task T1: Makefile: add LLAMA_CPP_TAG variable and print-llama-tag target
Status: complete
Branch: task/binary-bundling-T1
Done:
  - Added LLAMA_CPP_TAG ?= b5000 variable near top of Makefile
  - Added LLAMA_RELEASE_BASE computed from LLAMA_CPP_TAG
  - Added print-llama-tag phony target
  - Added print-llama-tag to .PHONY declaration
  - Verified: make -s print-llama-tag prints b5000
Issues found:
  - none
Contrarian verdict: approved

---

### Task T2: Makefile: add tarball target
Status: complete
Branch: task/binary-bundling-T2
Done:
  - Added TARBALL variable pointing to universal-apple-darwin tarball
  - Added tarball: target after release section with lipo merge of arm64+x86_64
  - Count-check verification for exactly one llama-server per arch
  - universal binary verification via file command
  - Copies sidecar from packaging/ into dist/ for tarball
  - Added deprecation comment and notice to brew-package
Issues found:
  - none
Contrarian verdict: approved

---

### Task T3: Makefile: update _pkg-layout to copy sidecar to SHARE_DIR
Status: complete
Branch: task/binary-bundling-T3
Done:
  - Added optional sidecar copy block in _pkg-layout after llama-server copy
  - chmod +x applied to sidecar in SHARE_DIR
  - Graceful degradation with WARN message if sidecar absent
Issues found:
  - none
Contrarian verdict: approved

---

### Task T4: packaging/homebrew/privacyclaw.rb: remove llama.cpp dependency and add llama-server install
Status: complete
Branch: task/binary-bundling-T4
Done:
  - Removed depends_on "llama.cpp" line
  - Updated on_macos URL to universal-apple-darwin tarball format
  - Added bin.install "llama-server" after bin.install "privacyclaw"
  - Added SOURCE OF TRUTH header comment with tap-sync-formula workflow
  - Updated README-style comments to reflect new tarball-based workflow
  - Fixed dependency comment to avoid substring match with "depends_on \"llama.cpp\""
Issues found:
  - Comment text "no depends_on \"llama.cpp\" needed" would have false-positively matched T9 test assertion. Fixed by rephrasing to "no llama.cpp dependency needed".
Contrarian verdict: approved

---

### Task T5: Makefile: add tap-sync-formula target and deprecate brew-package
Status: complete
Branch: task/binary-bundling-T5
Done:
  - Added tap-sync-formula to .PHONY declaration
  - Added tap-sync-formula target with SHA256 validation guard
  - Target copies source formula and substitutes PLACEHOLDER_SHA256 and version
  - Verified: make tap-sync-formula (no SHA256) exits 1 with usage message
  - Verified: make tap-sync-formula SHA256=testhash substitutes hash correctly
Issues found:
  - none
Contrarian verdict: approved

---

### Task T6: Sync tap formula: run tap-sync-formula with placeholder hash
Status: complete
Branch: task/binary-bundling-T6
Done:
  - Ran make tap-sync-formula SHA256=PLACEHOLDER_SHA256_REPLACE_BEFORE_PUBLISHING
  - Tap formula synced: universal-apple-darwin URL, llama-server bundled, no llama.cpp dep
  - Verified all required elements present: class Privacyclaw, bin.install "llama-server", depends_on "python@3.11", service do, test do
Issues found:
  - none
Contrarian verdict: approved

---

### Task T7: Create .github/workflows/release.yml
Status: complete
Branch: task/binary-bundling-T7
Done:
  - Created .github/workflows/release.yml with full release pipeline
  - macos-14 runner, Rust cache, version extraction from tag
  - Downloads arm64 and x86_64 llama-server zips, creates universal binary
  - Assembles tarball, creates GitHub Release via gh CLI
  - Syncs tap formula via tap-sync-formula, commits and pushes
  - YAML validated via Python
Issues found:
  - none
Contrarian verdict: approved

---

### Task T8: packaging/postinstall: add Python virtualenv creation after sidecar copy
Status: complete
Branch: task/binary-bundling-T8
Done:
  - Added venv creation block after sidecar copy (before ownership fixup)
  - Runs as actual user (su -m), best-effort with graceful degradation
  - Requirements: fastapi uvicorn httpx pydantic
  - Two warning levels: venv creation fail, pip install fail
  - bash -n syntax check passes
Issues found:
  - none
Contrarian verdict: approved

---

### Task T9: tests/brew_formula_test.rs: assert llama-server presence and no llama.cpp dependency
Status: complete
Branch: task/binary-bundling-T9
Done:
  - Added tap_formula_has_no_llama_cpp_dependency test
  - Added tap_formula_installs_llama_server test
  - Added source formula assertion to existing formula_privacyclaw_rb_exists_and_valid
  - Fixed tap_root() to use CARGO_MANIFEST_DIR directly (not parent) — existing bug fix
  - All 4 tests pass
Issues found:
  - tap_root() used CARGO_MANIFEST_DIR.parent() which was wrong for current repo structure (Cargo.toml at root). Fixed to CARGO_MANIFEST_DIR.join("homebrew-privacyclaw").
  - Formula comment "no depends_on \"llama.cpp\" needed" contained the searched substring — fixed in T4/T9.
Contrarian verdict: approved

---

### Task T10: tests/pkg_build_test.rs: assert sidecar copy step in Makefile
Status: complete
Branch: task/binary-bundling-T10
Done:
  - Added makefile_pkg_layout_copies_sidecar test
  - Added makefile_has_tarball_target test (also checks LLAMA_CPP_TAG, print-llama-tag, tap-sync-formula)
  - All 6 tests pass (4 existing + 2 new)
Issues found:
  - none
Contrarian verdict: approved

---

### Task T11: Makefile: add documentation comment to tap-update-version target
Status: complete
Branch: task/binary-bundling-T11
Done:
  - Updated comment above tap-update-version to clarify formula vs cask scope
  - Removed homebrew-privacyclaw/Formula/privacyclaw.rb from sed command
  - Only Casks/privacyclaw-app.rb is now patched by tap-update-version
Issues found:
  - none
Contrarian verdict: approved
