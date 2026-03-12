# Tasks: add-test-coverage

## 1. Test Infrastructure

- [x] 1.1 Add `tempfile = "3"` to `[dev-dependencies]` in `Cargo.toml` (confirmed present)
- [x] 1.2 Create shared test helpers inline in `#[cfg(test)]` blocks:
  - `make_anthropic_request(n_turns)` — builds realistic JSON body + HTTP envelope
  - `make_anthropic_sse_response(n_events)` — builds SSE HTTP response
  - `temp_store()` — opens a Store in a temp dir, returns `(Store, TempDir)`

## 2. Proxy Pipeline Tests (`src/proxy/intercept.rs`)

> **NOTE**: These tests were implemented in session 2026-03-08 but were subsequently
> removed when the `add-pii-protection` feature rewrote `intercept.rs`. They must be
> re-added against the new `intercept::run` signature (which includes `vault_registry`
> and `pii_config` parameters).

### 2.1 Roundtrip fidelity

- [x] 2.1.1 `test_small_request_response_forwarded_verbatim` — added to intercept.rs
- [x] 2.1.2 `test_large_request_response_forwarded_verbatim` — added to intercept.rs
- [x] 2.1.3 `test_proxy_does_not_modify_request_bytes` — added to intercept.rs
- [x] 2.1.4 `test_proxy_does_not_modify_response_bytes` — added to intercept.rs

### 2.2 Keep-alive / multi-turn

- [x] 2.2.1 `test_two_turns_on_one_connection` — implemented via run_keepalive helper
- [x] 2.2.2 `test_ten_turns_on_one_connection` — implemented via run_keepalive helper
- [x] 2.2.3 `test_per_request_state_reset` — implemented via run_keepalive helper
- [x] 2.2.4 `test_new_messages_only_stored_on_continuation` — implemented via run_keepalive helper

### 2.3 Concurrent sessions

- [x] 2.3.1 `test_two_concurrent_sessions_dont_interfere` — added to intercept.rs
- [x] 2.3.2 `test_twenty_concurrent_sessions` — added to intercept.rs
- [x] 2.3.3 `test_concurrent_same_fingerprint` — added to intercept.rs

### 2.4 Upstream failure modes

- [x] 2.4.1 `test_upstream_eof_mid_sse_finalizes_partial` — added to intercept.rs
- [x] 2.4.2 `test_upstream_immediate_eof_no_panic` — added to intercept.rs
- [x] 2.4.3 `test_client_disconnect_mid_response_still_stores` — added to intercept.rs
- [x] 2.4.4 `test_upstream_idle_timeout_fires` — implemented; UPSTREAM_READ_TIMEOUT shortened to 2 s in #[cfg(test)] builds

### 2.5 SSE streaming correctness

- [x] 2.5.1 `test_anthropic_message_stop_terminates_stream` — added to intercept.rs
- [x] 2.5.2 `test_openai_done_sentinel_terminates_stream` — added to intercept.rs
- [x] 2.5.3 `test_tokens_extracted_and_stored` — added to intercept.rs
- [x] 2.5.4 `test_sse_accumulation_buffer_cap` — added to intercept.rs
- [x] 2.5.5 `test_sse_split_across_tiny_chunks` — added to intercept.rs
- [x] 2.5.6 `test_ws_text_delta_events_fired` — added to intercept.rs
- [x] 2.5.7 `test_response_complete_ws_event_fired` — added to intercept.rs

### 2.6 Storage integration

- [x] 2.6.1 `test_new_conversation_created_on_first_request` — added to intercept.rs
- [x] 2.6.2 `test_conversation_start_ws_event_for_new_conv` — added to intercept.rs
- [x] 2.6.3 `test_response_stored_after_sse_complete` — added to intercept.rs
- [x] 2.6.4 `test_unparseable_request_no_storage_no_panic` — added to intercept.rs

## 3. Storage Tests (`src/storage/mod.rs`)

### 3.1 Basic CRUD

- [x] 3.1.1 `test_insert_and_get_conversation`
- [x] 3.1.2 `test_insert_and_get_messages_in_order`
- [x] 3.1.3 `test_batch_insert_preserves_order`
- [x] 3.1.4 `test_list_conversations_newest_first`

### 3.2 Fingerprinting

- [x] 3.2.1 `test_same_fingerprint_same_conv`
- [x] 3.2.2 `test_different_provider_same_fingerprint_separate_convs`
- [x] 3.2.3 `test_unknown_fingerprint_returns_none`
- [x] 3.2.4 `test_count_request_messages_counts_only_requests`

### 3.3 Concurrency

- [x] 3.3.1 `test_concurrent_batch_inserts_no_corruption`
- [x] 3.3.2 `test_write_lock_serializes_appends` — added to storage/mod.rs

### 3.4 Robustness

- [x] 3.4.1 `test_malformed_message_line_skipped`
- [x] 3.4.2 `test_batch_insert_missing_conv_is_noop`

## 4. Parser Tests (`src/parser/`)

### 4.1 Anthropic request parser (`src/parser/anthropic.rs`)

- [x] 4.1.1 `test_parse_model_and_messages`
- [x] 4.1.2 `test_parse_tool_use_content_array`
- [x] 4.1.3 `test_parse_image_block_elided`
- [x] 4.1.4 `test_parse_malformed_json_returns_none`
- [x] 4.1.5 `test_parse_missing_model_returns_none`

### 4.2 Anthropic SSE delta extraction

- [x] 4.2.1 `test_extract_delta_from_content_block_delta`
- [x] 4.2.2 `test_extract_tokens_from_message_start`
- [x] 4.2.3 `test_extract_tokens_from_message_delta`
- [x] 4.2.4 `test_empty_delta_returns_none`
- [x] 4.2.5 `test_non_delta_event_returns_none`

### 4.3 OpenAI parser

- [x] 4.3.1 `test_openai_parse_model_and_messages`
- [x] 4.3.2 `test_openai_extract_delta_from_choices`
- [x] 4.3.3 `test_openai_done_sentinel_is_none`

### 4.4 Google parser

- [x] 4.4.1 `test_google_parse_contents_field`
- [x] 4.4.2 `test_google_extract_delta_from_candidates`

### 4.5 Cross-provider

- [x] 4.5.1 `test_unknown_provider_falls_back_to_openai_format`
- [x] 4.5.2 `test_parse_request_scaling_182_turns_under_50ms`

## 5. SSE Parser Tests (`src/parser/sse.rs`)

- [x] 5.1 `test_lf_only_line_endings`
- [x] 5.2 `test_comment_lines_ignored`
- [x] 5.3 `test_multi_line_data_concatenated`
- [x] 5.4 `test_event_type_without_data_skipped`
- [x] 5.5 `test_1mb_data_line_no_panic` — added to sse.rs
- [x] 5.6 `test_1000_events_in_one_push`
- [x] 5.7 `test_byte_by_byte_same_as_full_push`
- [x] 5.8 `test_message_stop_event_parsed_correctly`

## 6. Network Helper Tests (`src/proxy/network.rs`)

- [x] 6.1 `test_peek_sni_extracts_hostname` — added to network.rs
- [x] 6.2 `test_peek_sni_returns_none_for_garbage` — added to network.rs
- [x] 6.3 `test_peek_sni_returns_none_for_truncated_buffer` — added to network.rs
- [x] 6.4 `test_intercept_decision_known_hosts` — added to network.rs
- [x] 6.5 `test_intercept_decision_unknown_host` — added to network.rs
- [x] 6.6 `test_dns_query_packet_format` — added to network.rs

## 7. Observability Tests (`src/util.rs`)

- [x] 7.1 `test_authorization_header_redacted`
- [x] 7.2 `test_x_api_key_header_redacted`
- [x] 7.3 `test_other_headers_not_redacted`
- [x] 7.4 `test_fmt_chunk_hex_truncates_at_256_bytes`
- [x] 7.5 `test_fmt_chunk_hex_short_input_not_truncated`
- [x] 7.6 `test_fmt_chunk_hex_empty_input`
