# Tasks: Fix T3-First PII Pipeline Correctness Bugs

## 1. Fix stale test (Root Cause E)

- [x] 1.1 In `tests/pii_config_tests.rs` at `patch_tier3_without_tier2_returns_error` (line 268):
  replace the test body so the initial config has `regex: false, ner: true, slm: false`
  and the patch enables `slm: true`. This combination (T2 without T1) is the actual invalid
  combo per the activation matrix. Assert `is_err()` and that the error message mentions the
  T1/T2 dependency (contains "tier 1", "tier1", "regex", or "requires").
  Rename the test to `patch_tier2_without_tier1_returns_error` or update the doc comment
  to accurately describe what is being tested.
  Verify: `cargo test patch_tier3_without_tier2_returns_error` (or renamed) passes.

## 2. Fix conv_id hardcoded as "conv" (Root Cause B)

- [x] 2.1 In `src/pii/mod.rs`, at the start of `process_request_body_async` (before the
  `entries` loop), read `conv_id` once from the vault:

  ```rust
  let conv_id = vault_handle.read().unwrap().conversation_id.clone();
  ```

  Remove all three occurrences of `"conv"` used as conversation identifier:

  - Line 219: `slm.replace(text, "conv", base_index)` → `slm.replace(text, &conv_id, base_index)`
  - Line 232: `let conv_id = "conv"; // placeholder` — remove; use the captured variable
  - Line 311: `let conv_id = "conv"; // placeholder` — remove; use the captured variable

  Verify: `cargo test` passes; the string literal `"conv"` no longer appears at these call sites.

## 3. Fix T3 original PII never captured (Root Cause A)

- [x] 3.1 In `src/pii/mod.rs`, Stage 1 right-to-left substitution loop (`mod.rs:228–236`):
  Before calling `result_text.replace_range(r.start..r.end, &xml)`, capture the original text:

  ```rust
  let original_text = result_text[r.start..r.end].to_string();
  ```

  This is safe because the loop runs right-to-left so `r.start..r.end` offsets are still
  valid at each iteration before `replace_range` is called.

  Update `spans_info` to a 5-tuple `Vec<(usize, usize, String, String, String)>` — add
  `original_text` as the fifth element alongside `display_value` and `pii_type`.
  Also update `EntryResult.stage1_spans` type annotation from
  `Vec<(usize, usize, String, String)>` to `Vec<(usize, usize, String, String, String)>`.

  In Phase 3 (around `mod.rs:312`), update the destructure pattern to include `original_text`:

  ```rust
  for (j, (_, _, display_val, pii_type_str, original_text)) in result.stage1_spans.iter().enumerate() {
  ```

  Pass `original_text` (instead of `&format!("T3_{j}")`) to `add_mapping_with_token_id` and
  to `PiiDetection.original`.
  Verify: `cargo test` passes; `"T3_0"` no longer appears as a vault original in any test output.

## 4. Fix cascade index maps skipped for T1/T2 spans (Root Cause C)

- [x] 4.1 In `src/pii/vault.rs`, refactor `add_mapping_with_token_id` (line 265–288):
  Compute `full_token` unconditionally before the guard (it is needed in both branches):

  ```rust
  let full_token = xml_token(token_id, display_value);
  if !self.original_to_synthetic.contains_key(original) {
      self.insert_mapping_raw_with_token_id(
          original.to_string(), display_value.to_string(),
          pii_type.label().to_string(), tier, confidence, token_id.to_string(),
      );
      self.rebuild_automaton();
      tracing::debug!(...);
  }
  // Always populate index maps (even on duplicate original — index maps are idempotent by key)
  self.full_token_to_original.insert(full_token, original.to_string());
  self.token_id_to_original.insert(token_id.to_string(), original.to_string());
  self.display_value_to_original.insert(display_value.to_string(), original.to_string());
  ```

  The three `HashMap::insert` calls are idempotent by key so re-insertion on true duplicates
  is safe.
  Verify: `cargo test` passes; specifically a scenario where `get_or_create` is called before
  `add_mapping_with_token_id` with the same original should now pass `get_by_token_id`.

## 5. Fix vault persistence drops token_id and display_value (Root Cause D)

- [x] 5.1 In `src/storage/mod.rs`, add two optional fields to `StoredVaultRecord`:

  ```rust
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub token_id: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub display_value: Option<String>,
  ```

  Update `save_vault` signature to accept
  `records: &[(String, String, String, u8, f32, String, String)]`
  (7-tuple: original, synthetic, pii_type, tier, confidence, token_id, display_value).
  In the mapping closure, populate `token_id` and `display_value` as `Some(...)` when non-empty
  or `None` when empty.
  Update all `save_vault` call sites in the storage test section of `mod.rs` to supply
  `String::new()` for the two new fields (backward-compatible, writes `None` to JSON).
  Verify: `cargo test -p privacyclaw --lib` passes; all existing vault storage tests pass.

- [x] 5.2 In `src/proxy/intercept/u2c.rs` (line 416–419), replace the `quints()`-based
  records collection with one that also captures `token_id` and `display_value`:

  ```rust
  let records: Vec<(String, String, String, u8, f32, String, String)> = vault
      .records()
      .into_iter()
      .map(|r| (
          r.original, r.synthetic, r.pii_type.label().to_string(),
          r.tier, r.confidence, r.token_id, r.display_value,
      ))
      .collect();
  ```

  (`vault.records()` returns `Vec<VaultRecord>` with all fields including `token_id` and
  `display_value`.)
  Verify: `cargo build` succeeds; the new 7-tuple type matches `save_vault`'s updated signature.

- [x] 5.3 In `src/pii/vault.rs`, in `load_or_create` (line 560–568), update the `VaultRecord`
  construction in the `map()` closure to pass `token_id` and `display_value` from the loaded
  `StoredVaultRecord`:

  ```rust
  token_id: r.token_id.unwrap_or_default(),
  display_value: r.display_value.unwrap_or_default(),
  ```

  Verify: `cargo test` passes; a round-trip test confirms that `get_by_token_id` returns
  the correct original after `load_or_create` when the stored record had a non-empty `token_id`.
