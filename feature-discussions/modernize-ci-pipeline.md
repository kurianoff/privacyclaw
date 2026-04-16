# Modernization: CI Pipeline — Add PR Gate Workflow

**Scope:** Add a GitHub Actions workflow that runs tests, clippy, rustfmt, and cargo-deny on every pull request and push to main.

## What and Why

The repository currently has a single workflow (`release.yml`) that fires only on version tags. There is no automated check for pull requests or pushes to main, which means formatting regressions, clippy warnings, and broken tests can land silently between releases. `cargo-deny` (license and security advisory checking) is also absent, creating a blind spot for supply-chain risk in a security-focused proxy tool. The release workflow itself does not run `cargo test` before building — it goes straight to `cargo build --release`.

## Key Items

- New workflow file `.github/workflows/ci.yml` triggering on `push` (branches: main) and `pull_request`
- Jobs: `fmt` (`cargo fmt --check`), `clippy` (`cargo clippy -- -D warnings`), `test` (`cargo test`), `deny` (`cargo deny check`)
- Add `cargo-deny` config (`deny.toml`) with license allowlist and advisory database check
- Optionally add `cargo-audit` as a separate scheduled weekly job for advisory scanning
- Cache Cargo registry and build artifacts using `actions/cache@v4` (pattern already exists in release.yml)
- Pin action versions (`actions/checkout@v4`, `dtolnay/rust-toolchain@stable`)

## Risks

Low. Additive only — no source changes. The main risk is discovering existing clippy warnings or test failures that must be fixed before the gate can be enabled. `cargo deny` may flag transitive deps with non-standard licenses requiring an allowlist entry.

## Dependencies on other blocks

None — can run independently, though running after `modernize-rust-edition-2024` avoids needing to update the workflow for edition changes.
