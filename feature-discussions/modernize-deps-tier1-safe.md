# Modernization: Dependency Bumps — Tier 1 (Safe, Non-Breaking)

**Scope:** Batch-bump all dependencies where a semver-compatible minor/patch update is available and no API migration is required.

## What and Why

Several dependencies are at non-latest minor or patch versions within their current semver range. Staying behind on minor/patch versions means missing bug fixes, performance improvements, and CVE patches. Because these are all within the same semver major as currently declared in Cargo.toml, `cargo update` can apply them without any source code changes — but it is worth running the test suite explicitly after the batch bump to confirm no behavioral regressions.

## Key Items

- `tracing-appender 0.2.4` — check for 0.2.x patch updates (tracing ecosystem)
- `tracing-subscriber 0.3.22` — check for 0.3.x patch updates
- `webpki-roots 0.26.11` — check for 0.26.x patch updates (root cert bundle freshness matters for TLS)
- `rcgen 0.13.2` — check for 0.13.x patch updates
- `rustls-pki-types 1.14.0` — check for 1.x patch updates
- `ring 0.17.14` — check for 0.17.x patch updates (cryptographic primitive library — important to stay current on patches)
- `fancy-regex 0.13.0` — check for 0.13.x patch updates
- `indicatif 0.17` / `dialoguer 0.11` — check for patch updates
- Run `cargo update` and `cargo test` to validate the batch

## Risks

Very low. All bumps are within declared semver ranges; no API changes expected. The only risk is an undiscovered behavioral difference in a patch release, caught by the test suite.

## Dependencies on other blocks

None — this block is purely additive and has no ordering requirements.
