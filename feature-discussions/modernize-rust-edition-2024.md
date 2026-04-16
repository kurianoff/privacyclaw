# Modernization: Rust Edition 2021 → 2024

**Scope:** Upgrade the crate from Rust edition 2021 to edition 2024 and pin a minimum toolchain version.

## What and Why

Rust edition 2024 was stabilized in Rust 1.85.0 (February 20, 2025). The current stable Rust as of April 2026 is 1.94.1 — meaning the project has been on 1.85+ for over a year and the edition upgrade has been available throughout. Edition 2024 brings improved `async fn` in traits (RPIT lifetime capture rules), `gen` blocks, `if let` temporary scope fixes, and stricter `unsafe` block requirements that help catch latent soundness issues. Staying on 2021 indefinitely means accumulating a migration debt as the ecosystem shifts conventions. There is also no `rust-toolchain.toml` file in the repo, so the effective toolchain version is silently floating — any developer or CI runner on a different Rust version gets an untested build.

## Key Items

- Cargo.toml `edition = "2021"` → `edition = "2024"`
- Add `rust-toolchain.toml` pinning `channel = "stable"` (current stable is 1.94.1 as of April 2026; pin to channel not a hardcoded version to stay evergreen)
- Run `cargo fix --edition` to auto-migrate mechanical changes (reserved keywords, `unsafe extern`, RPIT captures)
- Audit `unsafe` blocks flagged by the new edition's stricter lint
- Verify `async fn` in trait impls (e.g. `ResolvesServerCert`) still compile under the new lifetime capture semantics

## Risks

Low-to-medium. `cargo fix --edition` handles the majority of syntactic changes automatically. The main manual risk is `async fn` return-type lifetime capture changes in trait implementations — particularly the custom TLS resolver. Test suite provides a good safety net. No external API surfaces change.

## Dependencies on other blocks

None — can run first and unblocks cleaner code in all subsequent blocks.
