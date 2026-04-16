# Modernization: Replace fancy-regex with Pure-Rust Regex

**Scope:** Eliminate the `fancy-regex` + Oniguruma C library dependency by rewriting the two call sites to use the pure-Rust `regex` crate (already a direct dependency).

## What and Why

`fancy-regex` is used in exactly two places: `src/pii/tier1.rs` (one regex) and `src/pii/synth.rs` (one regex). The crate exists to support lookahead/lookbehind assertions. However, both patterns can likely be rewritten using the pure-Rust `regex` crate plus minor surrounding logic changes, eliminating a C FFI dependency (`onig` / Oniguruma) that adds build complexity, increases compile times, and requires a C toolchain on the build host. For a security-sensitive tool, reducing the C FFI surface is a meaningful risk reduction. The `regex` crate is already a direct dependency and is used extensively throughout the PII pipeline.

## Key Items

- Audit the two `fancy-regex` call sites to determine if lookahead/lookbehind is strictly necessary or can be replaced with split/anchor patterns
- If rewrite is straightforward: replace `fancy_regex::Regex` with `regex::Regex` at both sites, remove `fancy-regex` from Cargo.toml
- If lookahead is genuinely required at one site: evaluate `regex-syntax` extensions or restructure the match logic (e.g. capture-then-filter)
- Verify all PII detection test cases (`pii_roundtrip_test`, `pii_config_tests`) still pass after the change
- Confirm `onig` and `onig_sys` are fully evicted from the dependency tree post-removal

## Risks

Low-to-medium. The change is isolated to two small code sites. The primary risk is a subtle regex semantic difference (e.g., an overlapping match that fancy-regex handles via backtracking), which the existing PII roundtrip tests would catch. Compile time improvement will be measurable.

## Dependencies on other blocks

None — fully independent. Can run any time after the test suite is stable.
