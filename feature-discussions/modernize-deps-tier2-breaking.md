# Modernization: Dependency Bumps — Tier 2 (Breaking API Changes)

**Scope:** Migrate to newer major versions of dependencies that have breaking API changes requiring source edits.

## What and Why

Several direct dependencies have newer major versions available that the project is not yet on. The most significant are: `rand 0.8` (project) vs `rand 0.9` (already pulled in transitively by another dep, causing dual-version tree bloat); `tokio-tungstenite 0.24` vs the current 0.26 series; and `tokenizers 0.19` (used for the ORT/SLM feature) which may have newer releases. Carrying dual `rand` versions is particularly wasteful because the project itself uses `rand = "0.8"` while a transitive dependency has already pulled `rand 0.9` into the lockfile — migrating to `rand 0.9` as the direct dependency would consolidate the tree and eliminate the redundant copy.

## Key Items

- `rand 0.8.5` → `rand 0.9` (already in tree transitively — migrate direct usage; eliminates dual-version bloat). API change: `thread_rng()` removed, replaced by `rng()` / `rand::random()`
- `tokio-tungstenite 0.24` → `0.26` — WebSocket library major bump; check `Message` type and stream API changes in `src/dashboard/`
- `tokenizers 0.19` → latest stable (used in the optional `ort-ner` feature; verify NER pipeline API compatibility)
- `fancy-regex 0.13` → evaluate whether a major version has been released (lookahead/lookbehind usage is limited to two call sites in `src/pii/`)

## Risks

Medium. Each migration requires reading a changelog and updating call sites. `rand` migration affects `src/pii/synth.rs` and anywhere else `SmallRng` / `thread_rng` is used. WebSocket migration could affect the dashboard's live-update stream. Both are well-covered by integration tests. The `ort-ner` feature is optional and gated by a feature flag, reducing blast radius.

## Dependencies on other blocks

`modernize-deps-tier1-safe` should run first to avoid re-bumping a dep twice. `modernize-rust-edition-2024` is recommended first to avoid edition and API changes interleaving.
