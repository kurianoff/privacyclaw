# Modernization: Dependency Bumps — Tier 2 (Breaking API Changes)

**Scope:** Migrate to newer major versions of dependencies that have breaking API changes requiring source edits.

## What and Why

Several direct dependencies have newer major versions available that the project is not yet on. The most significant are: `rand 0.8` (project) vs `rand 0.9` already pulled transitively (causing dual-version tree bloat), and both versions being superseded by the newly released `rand 0.10.1` (February 2026); `tokio-tungstenite 0.24` vs the current 0.29.0; and `tokenizers 0.19` (used for the ORT/SLM feature) which may have newer releases. Carrying dual `rand` versions is particularly wasteful because the project itself uses `rand = "0.8"` while a transitive dependency has already pulled `rand 0.9` into the lockfile. Rather than migrating to 0.9 (itself already outdated), the project should target `rand 0.10` directly and consolidate the tree in one step.

## Key Items

- `rand 0.8.5` → `rand 0.10` (skip 0.9; 0.10.1 released February 2026 — current stable). API changes from 0.8: `thread_rng()` removed (replaced by `rng()` / `rand::random()`), `SmallRng` seeding API updated; review CHANGELOG for 0.9→0.10 additions
- `tokio-tungstenite 0.24` → `0.29` (current stable as of April 2026 — five major versions ahead). Check `Message` type variants, `WebSocketStream`, and handshake APIs used in `src/dashboard/`
- `tokenizers 0.19` → latest stable (used in the optional `ort-ner` feature; verify NER pipeline API compatibility)
- `fancy-regex 0.13` → `0.16` (current as of April 2026; actively maintained, multiple releases in 2025). API surface is small — two call sites in `src/pii/`

## Risks

Medium. Each migration requires reading a changelog and updating call sites. `rand` migration affects `src/pii/synth.rs` and anywhere else `SmallRng` / `thread_rng` is used. WebSocket migration could affect the dashboard's live-update stream. Both are well-covered by integration tests. The `ort-ner` feature is optional and gated by a feature flag, reducing blast radius.

## Dependencies on other blocks

`modernize-deps-tier1-safe` should run first to avoid re-bumping a dep twice. `modernize-rust-edition-2024` is recommended first to avoid edition and API changes interleaving.
