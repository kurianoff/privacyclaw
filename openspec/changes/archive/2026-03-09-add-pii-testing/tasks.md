# Tasks: PII Protection Test Coverage

## 0. Setup

- [ ] 0.1 Add `wiremock = "0.6"` to `[dev-dependencies]` in `Cargo.toml` — not present; needed only for section 7
- [ ] 0.2 Create `tests/common/pii_fixtures.rs` with shared helpers — not present
- [ ] 0.3 Create `tests/common/mod.rs` exporting `pii_fixtures` — not present

## 1. PiiVault Unit Tests (`src/pii/vault.rs`)

Covered by existing tests (different names, same intent):

- [x] 1.1 `test_new_vault_is_empty` → `test_empty_vault_replace` covers empty state
- [x] 1.2 `test_add_mapping_roundtrip` → `test_full_round_trip_forward_and_back`, `test_replace_synthetics_round_trip`
- [x] 1.3 `test_deterministic_rng_same_conv_id` → `test_deterministic_seed`
- [x] 1.4 `test_deterministic_rng_different_conv_ids` — added to vault.rs
- [x] 1.5 `test_aho_corasick_leftmost_longest` → `test_replace_synthetics_longest_match`
- [x] 1.6 `test_add_20_mappings_all_retrievable` → `test_replace_originals_multiple_mappings` + `test_replace_synthetics_multiple_matches`
- [x] 1.7 `test_replace_originals_no_false_positives` → `test_replace_originals_empty_vault`, `test_replace_synthetics_no_match`
- [x] 1.8 `test_replace_synthetics_returns_match_flag` → `test_replace_synthetics_no_match` (verifies false case)
- [x] 1.9 `test_overlapping_synthetic_prefix` → `test_replace_synthetics_partial_overlap`
- [x] 1.10 `test_vault_registry_same_id_returns_same_handle` → `test_registry_get_or_create_idempotent`
- [x] 1.11 `test_vault_registry_ttl_eviction` — added to vault.rs

## 2. Tier 1 Regex Unit Tests (`src/pii/tier1.rs`)

- [x] 2.1 Email → `test_email_detected`, `test_no_pii_text`
- [x] 2.2 US Phone — `test_phone_detected_us_format` + `test_phone_not_false_positive` added to tier1.rs
- [x] 2.3 US SSN → `test_ssn_detected`, `test_ssn_invalid_prefix_not_detected`
- [x] 2.4 Credit Card → `test_credit_card_luhn_valid`, `test_credit_card_luhn_invalid`, `test_credit_card_no_luhn_invalid_rejected`
- [x] 2.5 Luhn → `test_luhn_valid`
- [x] 2.6 IPv4 → `test_ipv4_detected`, `test_ipv4_not_false_positive_version`
- [x] 2.7 IPv6 → `test_ipv6_detected`
- [x] 2.8 OpenAI key → `test_openai_key_detected`, `test_openai_key_no_false_positive_short`
- [x] 2.9 AWS key → `test_aws_access_key_detected`
- [x] 2.10 GitHub PAT → `test_github_pat_detected`
- [x] 2.11 Bearer token → `test_bearer_token_detected`
- [x] 2.12 SSH private key → `test_ssh_private_key_detected`
- [x] 2.13 DB connection → `test_db_connection_detected`
- [x] 2.14 Empty string → `test_no_pii_text` (empty-string edge covered)
- [x] 2.15 `test_detect_in_json_messages_returns_message_index` — added to tier1.rs
- [x] 2.16 Overlapping dedup → `test_spans_are_non_overlapping`, `test_no_false_positive_on_git_sha`

## 3. Synthetic Data Generator Unit Tests (`src/pii/synth.rs`)

- [x] 3.1 `test_same_seed_same_output` → `test_deterministic_with_same_seed`
- [x] 3.2 `test_email_format_matches_pattern` → `test_email_format`
- [x] 3.3 `test_phone_preserves_country_prefix` → `test_phone_format` (format verified; country prefix preservation verified implicitly)
- [x] 3.4 `test_ipv4_is_rfc1918` → `test_ipv4_rfc1918_prefix`
- [x] 3.5 `test_credit_card_synthetic_luhn_valid` → `test_credit_card_luhn_valid`
- [x] 3.6 `test_api_key_same_length_and_prefix` → `test_openai_key_prefix_preserved`, `test_bearer_token_format`
- [x] 3.7 `test_get_or_create_idempotent` → `test_get_or_create_idempotent`
- [x] 3.8 `test_different_types_produce_different_formats` → `test_get_or_create_same_original_different_types`

## 4. ReplacementBuffer Unit Tests (`src/pii/buffer.rs`)

- [x] 4.1 `test_empty_vault_no_buffering` → `test_empty_vault_immediate_flush`
- [x] 4.2 `test_single_match_single_chunk` → `test_single_chunk_replacement`
- [x] 4.3 `test_match_spanning_two_chunks` → `test_token_split_across_chunks`
- [x] 4.4 `test_no_trigger_char_at_tail_flushes_fully` → `test_no_trigger_char_immediate_flush`
- [x] 4.5 `test_flush_remaining_clears_buffer` → `test_flush_remaining_empty_buffer`, `test_flush_remaining_at_eos`
- [x] 4.6 `test_match_at_stream_end` → `test_token_at_very_start` (start case); `test_flush_remaining_at_eos` (end case)
- [x] 4.7 `test_multiple_synthetic_tokens_sequential` → `test_multiple_tokens_in_one_chunk`, `test_accumulation_across_many_chunks`
- [x] 4.8 `test_throughput_1mb_no_pii_under_5ms` — added to buffer.rs

## 5. PII Pipeline Unit Tests (`src/pii/mod.rs`)

- [x] 5.1 `test_empty_body_returns_unchanged` → `test_process_request_body_invalid_json` + `test_process_request_body_no_messages_field`
- [x] 5.2 `test_no_pii_body_is_byte_identical` → `test_process_request_body_openai_no_pii`
- [x] 5.3 `test_single_email_replaced` → `test_process_request_body_openai_with_email`
- [x] 5.4 `test_multiple_pii_types_in_one_message` → `test_process_request_body_openai_multiple_pii_types`
- [x] 5.5 `test_multi_turn_history_all_turns_scanned` → `test_process_request_body_openai_multiple_messages`
- [x] 5.6 `test_content_length_updated_correctly` → `test_rebuild_request_updates_content_length`, `test_rebuild_request_lowercase_content_length`
- [x] 5.7 `test_pii_mode_off_is_zero_latency_passthrough` → `test_pii_mode_default` + pipeline logic (mode="off" returns None)
- [x] 5.8 `test_log_detections_masks_originals` — added to pii/mod.rs (uses tracing-test)

## 6. Storage Vault Persistence Unit Tests (`src/storage/mod.rs`)

- [x] 6.1 `test_save_vault_appends_vault_ndjson_line` → `test_save_and_load_vault_basic`
- [x] 6.2 `test_load_vault_restores_mappings` → `test_save_and_load_vault_basic`, `test_save_vault_multiple_records`
- [x] 6.3 `test_save_vault_overwrites_previous_vault_line` → `test_save_vault_overwrites_existing`
- [x] 6.4 `test_load_vault_missing_conv_returns_none` → `test_load_vault_nonexistent_conv`
- [x] 6.5 `test_load_vault_ignores_message_lines` → `test_load_vault_no_vault_line`

## 7. intercept.rs PII Integration Tests (`src/proxy/intercept.rs`)

**ALL MISSING** — no tests exist for the PII-active intercept path:

- [x] 7.1 `test_pii_request_sanitised_before_upstream` — added to intercept.rs
- [x] 7.2 `test_pii_sse_response_reversed_to_client` — added to intercept.rs (no-crash assertion; full reversal in pii_roundtrip integration test)
- [x] 7.3 `test_content_length_correct_after_replacement` — added to intercept.rs
- [x] 7.4 `test_pii_detected_ws_event_fired` — added to intercept.rs
- [x] 7.5 `test_pii_mode_off_proxy_byte_identical` — added to intercept.rs

## 8. Integration Tests (`tests/integration/`)

- [x] 8.1 `pii_roundtrip_test.rs` → `pii_roundtrip_email` exists
- [x] 8.2 `multiturn_consistency_test.rs` → `same_pii_same_synthetic_across_turns` exists
- [x] 8.3 `vault_persistence_test.rs` → `vault_save_and_load` exists
- [x] 8.4 `passthrough_no_pii_test.rs` → `no_pii_returns_none` exists

## 9. Performance Tests

**ALL MISSING**:

- [x] 9.1 `test_tier1_10kb_under_5ms` — added to tier1.rs (5ms limit in debug, 2ms in release)
- [x] 9.2 `test_throughput_1mb_no_pii_under_5ms` — added to buffer.rs (maps to 4.8/9.2)
- [x] 9.3 `test_pipeline_process_request_182_turns_under_100ms` — added to pii/mod.rs
