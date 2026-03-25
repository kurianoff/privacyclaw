# Tasks: update-pii-t3-first-pipeline

## 1. Vault: token_id support and new index structures

- [ ] 1.1 Add `token_id: String` and `display_value: String` fields to `VaultRecord` in `src/pii/vault.rs`, both with `#[serde(default)]` so legacy persisted records deserialize without error.
- [ ] 1.2 Add three new `HashMap<String, String>` fields to `PiiVault`: `full_token_to_original`, `token_id_to_original`, `display_value_to_original`. Initialize all to empty in `PiiVault::new()` and in `from_records()`.
- [ ] 1.3 Implement `fn generate_token_id(conversation_id: &str, entity_index: u64) -> String` in `src/pii/vault.rs`: SHA-256(conv_id + ":" + entity_index_decimal), first 6 bytes → base62 encode → 8 chars. Use `sha2::Sha256` (already in Cargo.toml). Base62 alphabet: `0-9A-Za-z`.
- [ ] 1.4 Implement `pub fn add_mapping_with_token_id(&mut self, original: &str, display_value: &str, token_id: &str, pii_type: PiiType, tier: u8, confidence: f32)` on `PiiVault`. This method: (a) calls the existing `add_mapping` logic to insert into all parallel vecs and rebuild the reverse automaton; (b) additionally populates `full_token_to_original`, `token_id_to_original`, and `display_value_to_original` HashMaps. The full XML token key is `format!("<pii id=\"{token_id}\">{display_value}</pii>")`.
- [ ] 1.5 Implement `pub fn get_by_token_id(&self, token_id: &str) -> Option<&str>` and `pub fn get_by_display_value(&self, display_value: &str) -> Option<&str>` on `PiiVault` (cascade Level 2 and Level 3 lookups).
- [ ] 1.6 Update `from_records()` to populate all three HashMaps when loading vault records that have a non-empty `token_id` field.
- [ ] 1.7 Add unit tests: `generate_token_id_deterministic` (same inputs → same output), `generate_token_id_distinct` (different entity_index → different token), `add_mapping_with_token_id_populates_all_maps`, `get_by_token_id_hit_and_miss`, `get_by_display_value_hit_and_miss`, `from_records_populates_new_maps_from_persisted_token_id`.

## 2. Config: rewrite validate_pii_tiers

- [ ] 2.1 In `src/config.rs`, rewrite `validate_pii_tiers` to: only reject `ner=true, regex=false` (T2 without T1). Remove the existing guard that rejects `{slm:true, regex:true, ner:false}` (T3+T1 without T2). Result: T3+T1, T3 alone, T3+T1+T2 are all valid.
- [ ] 2.2 Keep `is_t3_standalone` unchanged.
- [ ] 2.3 Update existing tests: `patch_tier3_without_tier2_is_error` must now use `{regex:false, ner:true, slm:false}` (T2 alone, no T1) to exercise the remaining rejection path. Add new test `patch_tier3_plus_t1_no_t2_is_valid`: assert `{regex:true, ner:false, slm:true}` passes validation.

## 3. Tier 3: add SlmSidecar::replace() and retire dead code

- [ ] 3.1 Add structs `ReplaceResponse` and `ReplaceReplacement` in `src/pii/tier3.rs` (both `#[derive(Deserialize)]`). Fields per design doc §4.3.
- [ ] 3.2 Implement `pub async fn replace(&self, text: &str, conversation_id: &str, entity_start_index: u64) -> Option<ReplaceResponse>` on `SlmSidecar`. POST to `{endpoint}/replace` with body `{"text": text, "conversation_id": conv_id, "entity_start_index": entity_start_index}`. On timeout, non-200, or JSON parse error: log `WARN`, return `None`. Timeout: reuse existing `reqwest::Client` timeout configured on `SlmSidecar`.
- [ ] 3.3 Delete `SYSTEM_PROMPT_STANDALONE`, `detect_and_rewrite`, and `extract_token_pairs` from `src/pii/tier3.rs`. These are dead code and violate the `clippy -D warnings` policy.
- [ ] 3.4 Add unit tests for `SlmSidecar::replace()` using `mockito` or `wiremock` (already used in the test suite): (a) success with valid `ReplaceResponse`; (b) timeout → returns `None`; (c) HTTP 500 → returns `None`; (d) malformed JSON → returns `None`.
- [ ] 3.5 Delete the five unit tests for `detect_and_rewrite` and the two tests for `extract_token_pairs`. Add equivalent coverage for `replace()` in their place (covered by 3.4).

## 4. Pipeline: restructure process_request_body_async for T3-first flow

- [ ] 4.1 Remove `pub slm_standalone: bool` from `PiiPipeline` struct in `src/pii/mod.rs` and all code that sets it (`PiiPipeline::tier1_only`, `PiiPipeline::new`).
- [ ] 4.2 Delete `process_body_t3_standalone` from `src/pii/mod.rs`.
- [ ] 4.3 Rewrite `process_request_body_async` in `src/pii/mod.rs` to implement the T3-first pipeline per design §5.1:
  - **Stage 1** (when `tiers.slm`): call `slm.replace(text, conv_id, entity_start_index).await`. On success: reconstruct modified text deterministically (right-to-left substitution using locally computed token IDs). Vault-insert each T3 replacement via `add_mapping_with_token_id`. Compute exclusion zones from reconstructed text. On failure: use raw text, no exclusion zones.
  - **Stage 2** (when `tiers.regex || tiers.ner`): call `detect_spans_with_exclusions(text, exclusion_zones)`. Merge T1+T2 spans. If `tiers.slm && tiers.ner`: call `slm.disambiguate` on low-confidence spans.
  - **Entity index pre-assignment**: before any vault write, determine `base_index = vault.mapping_count()`. Assign indices by sorted start-offset order (Stage 1 spans first, then Stage 2 spans in order).
  - **Vault write for Stage 2**: use `add_mapping_with_token_id` with locally computed token IDs.
  - **XML token assembly**: `format!("<pii id=\"{token_id}\">{display_value}</pii>")` replaces every detected span in the body text.
- [ ] 4.4 Implement `detect_spans_with_exclusions` (or add exclusion-zone filtering to the existing `detect_spans`): a span `[s, e]` is accepted iff for all exclusion zones `[s_i, e_i]`: `e <= s_i OR s >= e_i`.
- [ ] 4.5 Update `PiiDetection.synthetic` to carry the bare `display_value` (not the full XML token) for dashboard display. Only the request body text contains the XML token.
- [ ] 4.6 Update `SYSTEM_REMINDER` constant text in `src/pii/mod.rs` to describe the XML `<pii>` token format (per design §8.1).
- [ ] 4.7 Delete tests `pipeline_slm_standalone_flag_true_and_tier2_none` and `pipeline_full_stack_slm_standalone_false`. Add new tests for the tier-matrix routing logic: `pipeline_t3_only_calls_replace_not_t1t2`, `pipeline_t3_t1_calls_replace_then_t1`, `pipeline_t3_t1_t2_full_stack`.

## 5. Intercept: update system instruction gate

- [ ] 5.1 In `src/proxy/intercept/c2u.rs`, change the system-instruction injection gate from `p.pipeline.slm_standalone && p.mode == PiiMode::Replace` to simply `p.mode == PiiMode::Replace`. Delete the `slm_standalone` reference.
- [ ] 5.2 Verify `Content-Length` header is updated correctly after XML-token injection (existing `rebuild_request_with_content_length` call).

## 6. Buffer: XML-token holdback and cascade matcher

- [ ] 6.1 In `src/pii/buffer.rs`, add a second holdback trigger: scan for the literal 4-byte sequence `<pii` in the buffer. When found, hold back until `</pii>` is present.
- [ ] 6.2 Implement the cascade matcher for complete `<pii ...>...</pii>` tokens found in the buffer:
  - **Level 1**: lookup in `vault.full_token_to_original` (exact match on the full XML token string). O(1) HashMap lookup.
  - **Level 2**: extract `id="TOKEN_ID"`, call `vault.get_by_token_id(token_id)`.
  - **Level 3**: extract inner text (display value), call `vault.get_by_display_value(display_value)`.
  - **Level 4**: log `WARN "cascade Level 4 (hypothesis match) not yet implemented"`, pass token through unchanged. (Part II stub.)
  - On match at any level: replace the XML token with the original PII value.
- [ ] 6.3 Update the holdback window: the XML-token window size is `9 + 8 + 2 + max_display_value_len + 6` bytes (`<pii id="` + TOKEN_ID + `">` + display value + `</pii>`). Track `max_display_value_len` on `PiiVault` (or compute from `max_synthetic_key_len` since display values are the same strings). The overall safe flush length is `max(xml_window, display_value_window)`.
- [ ] 6.4 Ensure trigger_prefixes is built from display value 2-byte prefixes only (not XML token prefixes). The Level 5 Aho-Corasick (`reverse_automaton`) remains over display values unchanged. Confirm `['<', 'p']` is NOT added to `trigger_prefixes` as a result of any XML-token entry.
- [ ] 6.5 Add unit tests: `buffer_xml_token_reversed_level1` (exact match), `buffer_xml_token_reversed_level2` (token_id only matches), `buffer_xml_token_reversed_level3` (display_value only matches), `buffer_xml_token_passthrough_level4` (no match → WARN and passthrough), `buffer_xml_token_split_across_chunks` (partial `<pii` in one chunk, rest in next), `buffer_trigger_prefixes_no_xml_prefix` (assert `[b'<', b'p']` not in trigger set after vault insert).

## 7. Token format: XML token assembly helpers

- [ ] 7.1 In `src/pii/vault.rs` (or a shared util), implement `pub fn xml_token(token_id: &str, display_value: &str) -> String` returning `format!("<pii id=\"{token_id}\">{display_value}</pii>")`. Use at all call sites where the XML token string is constructed.
- [ ] 7.2 Verify `synth.rs` signature is unchanged — `SyntheticGenerator::get_or_create` still returns a bare display-value `String`. XML wrapping happens only at call sites in `replace_with_spans` and Stage 1 vault insert, never inside `synth.rs`.

## 8. Packaging: postinstall update

- [ ] 8.1 In `packaging/postinstall`, extend the install script to copy `privacyclaw-slm-sidecar` (Python script) to the application support bin directory alongside `llama-server`. Add an existence check: if the script is not present in `$SHARE_DIR`, skip with a log message (sidecar is optional — proxy starts without it).
- [ ] 8.2 Update the Homebrew formula comment / description to note the Python sidecar dependency (the sidecar implementation itself is out of scope for this change).

## 9. Spec deltas: design.md copy

- [ ] 9.1 Copy `.claude/workflow/pii-t3-first-pipeline/design.md` to `docs/pii-pipeline-v2.md` for long-term reference.

## 10. Final validation

- [ ] 10.1 Run `cargo test` and fix all compilation errors and test failures. Ensure all new tests pass and all deleted tests are removed.
- [ ] 10.2 Run `cargo clippy -- -D warnings` and fix all warnings. Pay special attention to any remaining references to `slm_standalone` or `detect_and_rewrite` / `extract_token_pairs`.
- [ ] 10.3 Run `openspec validate update-pii-t3-first-pipeline --strict` and resolve any validation issues.
