# Modernization: ORT Crate — Advance RC Pin to Latest RC

**Scope:** Migrate the `ort` dependency from the stale pinned release-candidate `=2.0.0-rc.10` to the latest available RC, and establish a policy for tracking future RC advances until 2.0 stable ships.

## What and Why

The `ort` crate (ONNX Runtime Rust bindings) is pinned with an exact version constraint (`=2.0.0-rc.10`). As of April 2026, ort 2.0 stable has NOT been released — the project is still in release candidate phase, now at `2.0.0-rc.12` (released March 5, 2026). The exact-pin means `cargo update` can never advance it, so the project is stuck on rc.10 (June 2025) while rc.11 (January 2026) and rc.12 (March 2026) contain fixes. Each RC is described by the maintainer as "production-ready but not API stable." The `=` pin is necessary because RCs do not follow semver compatibility guarantees between them, so the migration must be done deliberately with API review.

## Key Items

- `ort = "=2.0.0-rc.10"` → `ort = "=2.0.0-rc.12"` (current latest as of April 2026)
- Review the rc.10 → rc.11 → rc.12 changelogs for `Session`, `Tensor`, `Inputs`, and execution provider API changes affecting `src/models/`
- `download-binaries` feature: verify rc.12 downloads a compatible ORT runtime shared library version
- `tokenizers = "0.19"` — check if rc.12 requires a companion tokenizers version bump
- Monitor the pykeio/ort GitHub releases page; when 2.0.0 stable ships, a separate migration pass will be needed to switch from `"=2.0.0-rc.X"` to `"2"`
- Update `ndarray = "0.16"` if the new RC requires a different ndarray version

## Risks

Low-to-medium. The changes are confined to the optional `ort-ner` feature flag, so the main proxy pipeline is unaffected. RC-to-RC migrations within the 2.0 series are smaller in scope than an RC-to-stable migration. The `t3_standalone_roundtrip` test provides a regression gate. The risk of API breakage between rc.10 and rc.12 is real but bounded.

## Dependencies on other blocks

`modernize-deps-tier2-breaking` should run first since it may also touch `tokenizers` and `ndarray`. Does not depend on edition upgrade.
