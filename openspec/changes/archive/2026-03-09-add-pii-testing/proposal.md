# Change: Add PII Protection Test Coverage

## Why

The `add-pii-protection` change introduces six new modules (`pii/vault`, `pii/tier1`, `pii/synth`, `pii/buffer`, `pii/mod`, `storage` vault persistence) and modifies the critical hot path in `intercept.rs`. Without systematic test coverage the three hard correctness invariants — zero PII leaves the machine, round-trip fidelity for the client, and per-conversation mapping idempotency — cannot be mechanically verified. This change specifies the full test suite that enforces those invariants at unit, integration, and performance level.

## What Changes

- **New spec capability**: `pii-testing` — requirements and scenarios for every test category
- **New files**: `tests/integration/` directory with four integration test files
- **Modified files**: inline `#[cfg(test)]` blocks added to `src/pii/vault.rs`, `src/pii/tier1.rs`, `src/pii/synth.rs`, `src/pii/buffer.rs`, `src/pii/mod.rs`, `src/storage/mod.rs`, `src/proxy/intercept.rs`
- **New test helper**: `tests/common/pii_fixtures.rs` with shared builders and assertion helpers
- **New dev-dependency**: `wiremock = "0.6"` for Tier 3 SLM mock HTTP server tests

## Impact

- **New specs**: `pii-testing`
- **Affected specs**: none (this change only adds tests; production behaviour is unchanged)
- **Affected code**:
  - `src/pii/vault.rs` — inline unit tests
  - `src/pii/tier1.rs` — inline unit tests (all entity types)
  - `src/pii/synth.rs` — inline unit tests
  - `src/pii/buffer.rs` — inline unit tests
  - `src/pii/mod.rs` — inline unit tests (pipeline orchestration)
  - `src/storage/mod.rs` — vault persistence unit tests (appended to existing test module)
  - `src/proxy/intercept.rs` — PII integration tests (appended to existing test module)
  - `tests/integration/` — four new integration test files
  - `tests/common/pii_fixtures.rs` — shared helpers
  - `Cargo.toml` — `wiremock` dev-dependency
