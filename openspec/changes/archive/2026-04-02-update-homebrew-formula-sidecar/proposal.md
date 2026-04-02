# Change: Update Homebrew Formula to Package Python SLM Sidecar

## Why

The T3 PII pipeline ships a Python sidecar (`privacyclaw-slm-sidecar`) that is
not yet distributed via Homebrew. Users who install via `brew install privacyclaw`
have no access to the sidecar or its pip dependencies, and cannot invoke it for
debugging or enhanced PII detection. This change closes that gap.

## What Changes

- **`packaging/privacyclaw-slm-sidecar`**: add `--version` / `-V` flag handled
  before the dependency guard, printing `privacyclaw-slm-sidecar 0.1.0` and
  exiting 0. No new imports required (`sys` is already imported).

- **`packaging/homebrew/privacyclaw.rb`**:
  - Add `depends_on "python@3.11"` alongside the existing `depends_on "llama.cpp"`
  - Add `resource` blocks for all direct and required transitive pip dependencies
    with pinned versions and SHA-256 checksums. `pydantic-core` uses
    platform-specific `on_macos { on_arm { } }` / `on_intel { }` wheel blocks
    to avoid requiring a Rust toolchain at install time.
  - Update the `install` block: call `virtualenv_install_with_resources` to
    create a managed virtualenv at `libexec/`, then `bin.install` the sidecar
    script and rewrite its shebang to `opt_libexec/bin/python3` via `inreplace`.
  - Expand the `caveats` block with T3 model setup instructions and sidecar
    usage documentation.
  - Add a sidecar smoke test (`--version` assertion) to the `test` block.

## Impact

- Affected specs: `packaging` (new capability delta)
- Affected code: `packaging/privacyclaw-slm-sidecar`, `packaging/homebrew/privacyclaw.rb`
- No Rust source changes. No tap formula changes (out of scope).
- Pip resource checksums are resolved at implementation time via `pip download`.
