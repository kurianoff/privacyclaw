# Implementation Log: update-pii-t3-first-pipeline

Feature: Implement Part I of the Adaptive PII Protection: T3-First Pipeline
Branch: feature/pii-t3-first-pipeline
Started: 2026-03-25

---

## Task Groups

- Group 1: Vault index structures (tasks 1.1–1.7)
- Group 2: Config validation (tasks 2.1–2.3)
- Group 3: Tier3 replace/dead-code (tasks 3.1–3.5)
- Group 4: Pipeline restructure (tasks 4.1–4.7)
- Group 5: Intercept gate (tasks 5.1–5.2)
- Group 6: Buffer cascade (tasks 6.1–6.5)
- Group 7: Token helpers (tasks 7.1–7.2)
- Group 8: Packaging (tasks 8.1–8.2)
- Group 9: Doc copy (task 9.1)
- Group 10: Final validation (tasks 10.1–10.3)

---

### Task Group 1: Vault token_id support and new index structures
Status: complete
Branch: feature/pii-t3-first-pipeline
Done:
  - `token_id` and `display_value` fields added to `VaultRecord` with `#[serde(default)]`
  - Three index HashMaps added to `PiiVault`: `full_token_to_original`, `token_id_to_original`, `display_value_to_original`
  - `generate_token_id` implemented using SHA-256 + base62 encoding
  - `xml_token` helper implemented
  - `add_mapping_with_token_id` implemented, populates all three indexes
  - `get_by_token_id` and `get_by_display_value` implemented
  - `from_records` updated to populate index maps for records with token_id
  - All 6 unit tests from task 1.7 present and passing
Issues found:
  - none
Contrarian verdict: approved

### Task Group 2: Config validation rewrite
Status: complete
Branch: feature/pii-t3-first-pipeline
Done:
  - `validate_pii_tiers` rewritten: only T2-without-T1 rejected; T3+T1 valid
  - `is_t3_standalone` unchanged
  - `patch_tier3_without_tier2_is_error` updated to use T2-without-T1 pattern
  - `patch_tier3_plus_t1_no_t2_is_valid` added
Issues found:
  - none
Contrarian verdict: approved

### Task Group 3: Tier3 replace/dead-code retirement
Status: complete
Branch: feature/pii-t3-first-pipeline
Done:
  - `ReplaceResponse` and `ReplaceReplacement` structs added
  - `SlmSidecar::replace()` implemented with timeout/500/JSON-error handling
  - `SYSTEM_PROMPT_STANDALONE`, `detect_and_rewrite`, `extract_token_pairs` deleted
  - replace() tests: success, timeout→None, HTTP500→None, malformed_json→None
Issues found:
  - none
Contrarian verdict: approved

### Task Group 4: Pipeline restructure
Status: complete
Branch: feature/pii-t3-first-pipeline
Done:
  - `pub slm_standalone: bool` removed from `PiiPipeline`
  - `process_body_t3_standalone` removed
  - `process_request_body_async` rewritten for T3-first flow (Stage 1+Stage 2)
  - `detect_spans_with_exclusions` implemented
  - `replace_with_spans_xml` implemented (XML token format)
  - `SYSTEM_REMINDER` updated to XML token format
  - Pipeline tests: `pipeline_t3_only_tier_matrix_routing`, `pipeline_t3_plus_t1_tier_matrix_routing`, `pipeline_t3_t1_t2_full_stack_routing`
Issues found:
  - `replace_with_spans` remains (unused, triggers clippy warning) — address in task 10.2
Contrarian verdict: approved

### Task Group 5: Intercept gate
Status: complete
Branch: feature/pii-t3-first-pipeline
Done:
  - `c2u.rs` gate changed from `slm_standalone && Replace` to `Replace` only
  - `slm_standalone` reference removed
Issues found:
  - none
Contrarian verdict: approved

### Task Group 7: Token format helpers
Status: complete
Branch: feature/pii-t3-first-pipeline
Done:
  - `xml_token` implemented in `vault.rs`
  - `synth.rs` signature unchanged (returns bare display-value)
Issues found:
  - none
Contrarian verdict: approved

### Task Group 6: Buffer XML-token holdback and cascade (RESUMING)
Status: in-progress
Branch: feature/pii-t3-first-pipeline

### Task Group 8: Packaging postinstall update (RESUMING)
Status: in-progress
Branch: feature/pii-t3-first-pipeline

### Task Group 9: Doc copy (RESUMING)
Status: in-progress
Branch: feature/pii-t3-first-pipeline

### Task Group 10: Final validation (RESUMING)
Status: in-progress
Branch: feature/pii-t3-first-pipeline
