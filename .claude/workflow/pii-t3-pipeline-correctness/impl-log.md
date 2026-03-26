# Implementation Log: Fix T3-First PII Pipeline Correctness Bugs

Branch: fix/pii-t3-pipeline-correctness
OpenSpec ID: fix-pii-t3-pipeline-correctness
Commit: e85ed9d

## Task Status Summary

| Task | Title | Status |
|------|-------|--------|
| 1.1  | Fix stale test (Root Cause E) | complete |
| 2.1  | Fix conv_id hardcoded as "conv" (Root Cause B) | complete |
| 3.1  | Fix T3 original PII never captured (Root Cause A) | complete |
| 4.1  | Fix cascade index maps skipped for T1/T2 spans (Root Cause C) | complete |
| 5.1  | Add token_id/display_value to StoredVaultRecord (Root Cause D, storage) | complete |
| 5.2  | Update u2c.rs vault persist call site (Root Cause D, call site) | complete |
| 5.3  | Update vault load_or_create (Root Cause D, load path) | complete |

---

### Task 1.1: Fix stale test (Root Cause E)
Status: complete
Branch: fix/pii-t3-pipeline-correctness (direct, no worktree)
Done:
  - Changed `patch_tier3_without_tier2_returns_error` in `tests/pii_config_tests.rs`:
    initial config now `{regex:false, ner:true, slm:false}`, patch `{slm:true}`
  - Added assertion that error message mentions T1/T2 dependency (contains "tier 1", "tier1", "regex", or "requires")
Issues found:
  - none
Contrarian verdict: approved (inline review)

---

### Task 2.1: Fix conv_id hardcoded as "conv" (Root Cause B)
Status: complete
Branch: fix/pii-t3-pipeline-correctness (direct, no worktree)
Done:
  - Added `PiiVault::conversation_id()` public accessor (field was private)
  - Read conv_id once at top of `process_request_body_async` via `vault_handle.read().unwrap().conversation_id().to_string()`
  - Replaced all three "conv" literals in mod.rs: Stage 1 slm.replace call, Stage 1 token_id generation, Stage 3 generate_token_id call
Issues found:
  - `conversation_id` field is private — required adding a public getter method. Minimal change.
Contrarian verdict: approved (inline review)

---

### Task 3.1: Fix T3 original PII never captured (Root Cause A)
Status: complete
Branch: fix/pii-t3-pipeline-correctness (direct, no worktree)
Done:
  - Captured `original_text = result_text[r.start..r.end].to_string()` before `replace_range`
  - Expanded `spans_info` / `EntryResult.stage1_spans` from 4-tuple to 5-tuple adding `original_text`
  - Updated Stage 3 destructure pattern to `(_, _, display_val, pii_type_str, original_text)`
  - Pass `original_text` to `add_mapping_with_token_id` and `PiiDetection.original`
  - Updated exclusion zone loop to destructure 5-tuple
Issues found:
  - none
Contrarian verdict: approved (inline review)

---

### Task 4.1: Fix cascade index maps skipped for T1/T2 spans (Root Cause C)
Status: complete
Branch: fix/pii-t3-pipeline-correctness (direct, no worktree)
Done:
  - Restructured `add_mapping_with_token_id` in `src/pii/vault.rs`:
    compute `full_token` unconditionally before the guard
    move core insert + automaton rebuild + DEBUG log inside `if !contains_key` branch
    add else-branch DEBUG log for duplicate case
    always execute three index HashMap inserts after the guard
Issues found:
  - none
Contrarian verdict: approved (inline review)

---

### Task 5.1: Add token_id/display_value to StoredVaultRecord (Root Cause D, storage)
Status: complete
Branch: fix/pii-t3-pipeline-correctness (direct, no worktree)
Done:
  - Added `token_id: Option<String>` and `display_value: Option<String>` to `StoredVaultRecord`
    with `#[serde(default, skip_serializing_if = "Option::is_none")]` for backward compat
  - Extended `save_vault` signature from 5-tuple to 7-tuple records
  - Updated mapping closure to populate token_id/display_value as Some(...) when non-empty, None when empty
  - Updated all 5 test call sites in storage/mod.rs + 2 in vault_confidence_test.rs + 1 in detection_log_test.rs + 1 in vault_persistence_test.rs + 3 StoredVaultRecord struct literals
Issues found:
  - More test call sites than expected (external test files in tests/ also used the 5-tuple)
Contrarian verdict: approved (inline review)

---

### Task 5.2: Update u2c.rs vault persist call site (Root Cause D, call site)
Status: complete
Branch: fix/pii-t3-pipeline-correctness (direct, no worktree)
Done:
  - Changed `vault.quints().map(...)` to `vault.to_records().into_iter().map(|r| ...)` in `src/proxy/intercept/u2c.rs`
  - New 7-tuple includes token_id and display_value from VaultRecord
Issues found:
  - Method is named `to_records()` not `records()` (as mentioned in task spec)
Contrarian verdict: approved (inline review)

---

### Task 5.3: Update vault load_or_create (Root Cause D, load path)
Status: complete
Branch: fix/pii-t3-pipeline-correctness (direct, no worktree)
Done:
  - Updated VaultRecord construction in `load_or_create` to pass
    `token_id: r.token_id.unwrap_or_default()` and `display_value: r.display_value.unwrap_or_default()`
    instead of hardcoded `String::new()`
Issues found:
  - none
Contrarian verdict: approved (inline review)

---

## Final Test Results

- All 383 lib tests pass
- All 385 bin tests pass
- All integration tests pass (vault_persistence_test, detection_log_test, vault_confidence_test, pii_config_tests, t3_standalone_roundtrip)
- 2 pre-existing failures in brew_formula_test (Homebrew formula files not present on this machine, unrelated to our changes)

