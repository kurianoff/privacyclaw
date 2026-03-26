# Change: Fix T3-First PII Pipeline Correctness Bugs

## Why

The T3-first PII pipeline (merged in `feature/pii-t3-first-pipeline`) has five
production-blocking bugs discovered by RCA. The most severe: T3-detected PII is
irreversible because the vault stores `"T3_0"` placeholder strings instead of the
real originals. Secondary bugs: `conv_id` hardcoded as `"conv"` defeats per-conversation
token scoping; cascade L1/L2/L3 index maps are silently empty for T1/T2 spans due to
an over-eager idempotent guard; `token_id` and `display_value` are never written to
storage so cascade lookups fail after proxy restart; one test asserts the wrong
error for a now-valid tier combination.

## What Changes

- `src/pii/mod.rs`: Capture `&text[r.start..r.end]` as the vault original before
  `replace_range` mutates the string (Root Cause A). Read `conversation_id` from vault
  once at entry to `process_request_body_async` and pass to `generate_token_id` instead
  of the hardcoded literal `"conv"` (Root Cause B).
- `src/pii/vault.rs`: Split the idempotent guard in `add_mapping_with_token_id` so
  the `original_to_synthetic` insert is still skipped on duplicate but the three
  index HashMaps (`full_token_to_original`, `token_id_to_original`,
  `display_value_to_original`) are always populated (Root Cause C). Update `load_or_create`
  to pass loaded `token_id` and `display_value` from storage into `VaultRecord` (Root Cause D,
  load side).
- `src/storage/mod.rs`: Add `token_id` and `display_value` as optional fields to
  `StoredVaultRecord` with `#[serde(default, skip_serializing_if = "Option::is_none")]`
  for backward compatibility; extend `save_vault` to accept and persist these fields;
  update `load_vault` return path to surface them (Root Cause D, storage side).
- `src/proxy/intercept/u2c.rs`: Update the `records` collection at vault persist time
  to include `token_id` and `display_value` from the vault (Root Cause D, call site).
- `tests/pii_config_tests.rs`: Fix `patch_tier3_without_tier2_returns_error` to test
  the actual invalid combo `{regex:false, ner:true, slm:false}` (Root Cause E).

## Impact

- Affected specs: `pii-vault`, `pii-pipeline`
- Affected code: `src/pii/mod.rs`, `src/pii/vault.rs`, `src/storage/mod.rs`,
  `src/proxy/intercept/u2c.rs`, `tests/pii_config_tests.rs`
- Not breaking: `StoredVaultRecord` gains optional fields with `#[serde(default)]` —
  existing NDJSON vault entries load correctly with `token_id: None`, `display_value: None`
- No new Cargo dependencies
