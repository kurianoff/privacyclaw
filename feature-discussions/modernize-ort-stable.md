# Modernization: ORT Crate — Exit Release Candidate Pin

**Scope:** Migrate the `ort` dependency from the pinned release-candidate `=2.0.0-rc.10` to a stable 2.x release.

## What and Why

The `ort` crate (ONNX Runtime Rust bindings) is pinned with an exact version constraint (`=2.0.0-rc.10`), indicating the project adopted it before the stable 2.0 series was released. ORT 2.0 stable was published in late 2024 with breaking changes over the RC. Remaining on an RC version indefinitely means: no security patches from the maintainer; potential incompatibility with newer ONNX Runtime shared library releases; and the `=` pin prevents `cargo update` from ever resolving it. For a privacy-focused tool where the NER pipeline processes sensitive user data, running an unmaintained RC is a material risk.

## Key Items

- `ort = "=2.0.0-rc.10"` → `ort = "2"` (or the current stable patch, e.g. `2.0.x`)
- Audit the stable 2.0 changelog for API changes in `Session`, `Tensor`, and execution provider APIs used in `src/models/`
- `download-binaries` feature: verify the stable release still supports this feature and downloads a compatible ORT runtime version
- `tokenizers = "0.19"` — check if the companion crate version needs a corresponding bump for the stable ORT
- Update `ndarray = "0.16"` if ORT stable has a dependency on a newer ndarray

## Risks

Medium-high. The `ort` 2.0 stable series introduced breaking API changes from the RC. The affected code is isolated to the optional `ort-ner` feature (src/models/), so the main proxy pipeline is unaffected. Requires careful changelog review and testing the NER pipeline end-to-end. The `t3_standalone_roundtrip` test provides a regression gate.

## Dependencies on other blocks

`modernize-deps-tier2-breaking` should run first since it may also touch `tokenizers` and `ndarray`. Does not depend on edition upgrade.
