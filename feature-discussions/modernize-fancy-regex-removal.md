# Modernization: Replace fancy-regex with Pure-Rust Regex

**Scope:** Remove the `fancy-regex` dependency by rewriting the two call sites to use the pure-Rust `regex` crate (already a direct dependency).

## What and Why

`fancy-regex` is used in exactly two places: `src/pii/tier1.rs` (one regex) and `src/pii/synth.rs` (one regex). The crate exists to support lookahead/lookbehind assertions. Both patterns can likely be rewritten using the `regex` crate plus minor surrounding logic changes. Importantly, fancy-regex is NOT a C FFI dependency — it is pure Rust and uses the `regex` crate as its underlying engine, delegating only the "fancy" backtracking portions to its own implementation. The motivation for removal is therefore not C-safety but dependency weight and API simplification: fancy-regex 0.13 → 0.16 is itself a breaking change (see `modernize-deps-tier2-breaking`), and if the lookahead patterns can be eliminated, the crate can be dropped entirely rather than migrated. As of April 2026, fancy-regex is at 0.16.2 and actively maintained (multiple releases in 2025), so keeping it is also a valid choice if the rewrite proves complex.

## Key Items

- Audit the two `fancy-regex` call sites to determine if lookahead/lookbehind is strictly necessary or can be replaced with split/anchor/capture patterns in `regex`
- If rewrite is straightforward: replace `fancy_regex::Regex` with `regex::Regex` at both sites, remove `fancy-regex` from Cargo.toml
- If lookahead is genuinely required at one site: migrate to `fancy-regex 0.16` in `modernize-deps-tier2-breaking` instead of removing, and close this block as superseded
- Verify all PII detection test cases (`pii_roundtrip_test`, `pii_config_tests`) still pass after any change
- Confirm `fancy-regex` is fully removed from the dependency tree if the rewrite path is chosen

## Risks

Low-to-medium. The change is isolated to two small code sites. The primary risk is a subtle regex semantic difference (e.g., an overlapping match that fancy-regex handles via backtracking), which the existing PII roundtrip tests would catch. If removal proves impractical, the fallback is a straightforward version bump within `modernize-deps-tier2-breaking`.

## Dependencies on other blocks

None — fully independent. Can run any time after the test suite is stable.
