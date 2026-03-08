# Tasks: add-test-coverage

## 1. Test Infrastructure

- [ ] 1.1 Add `tracing-subscriber` and `uuid` to `[dev-dependencies]` in `Cargo.toml` (already present; confirm)
- [ ] 1.2 Create shared test helpers module `src/proxy/intercept/test_helpers.rs` (or inline in `#[cfg(test)]`):
  - `make_anthropic_request(n_turns)` — builds realistic JSON body + HTTP envelope
  - `make_anthropic_sse_response(n_events, chars_per_event)` — builds SSE HTTP response
  - `make_openai_request(n_turns)` / `make_openai_sse_response(n_events)`
  - `make_json_response(body_json)` — non-SSE HTTP/1.1 200 response with Content-Length
  - `run_proxy_once(request_bytes, response_bytes)` — wires four duplex pipes, runs `intercept::run`, returns `(forwarded_request, forwarded_response, store)`
  - `temp_store()` — opens a Store in a temp dir, returns `(Store, TempDir)`

## 2. Proxy Pipeline Tests (`src/proxy/intercept.rs`)

### 2.1 Roundtrip fidelity

- [ ] 2.1.1 `test_small_request_response_forwarded_verbatim` — 1-turn request + JSON response: assert forwarded request bytes == sent bytes, forwarded response bytes == upstream bytes
- [ ] 2.1.2 `test_large_request_response_forwarded_verbatim` — 40-turn request (~600 KB) + SSE response: same assertion; regression test for TLS flush fix
- [ ] 2.1.3 `test_proxy_does_not_modify_request_bytes` — XOR all sent vs forwarded bytes; assert identical
- [ ] 2.1.4 `test_proxy_does_not_modify_response_bytes` — same for response direction

### 2.2 Keep-alive / multi-turn

- [ ] 2.2.1 `test_two_turns_on_one_connection` — send request 1 + response 1, then request 2 + response 2 on same pipe pair; both responses received correctly
- [ ] 2.2.2 `test_ten_turns_on_one_connection` — 10 sequential request/response pairs; all forwarded correctly, all stored
- [ ] 2.2.3 `test_per_request_state_reset` — second request has different Content-Length from first; assert correct body_bytes logged for both
- [ ] 2.2.4 `test_new_messages_only_stored_on_continuation` — turn 2 request includes turn 1 messages; assert only the new message is appended to storage

### 2.3 Concurrent sessions

- [ ] 2.3.1 `test_two_concurrent_sessions_dont_interfere` — two `intercept::run` tasks running simultaneously with different requests; each gets its own response
- [ ] 2.3.2 `test_twenty_concurrent_sessions` — 20 tasks, each a 3-turn conversation; all complete within 5s, storage has correct total message count
- [ ] 2.3.3 `test_concurrent_same_fingerprint` — two sessions share the same first message; both resolve to the same conv_id, storage not corrupted

### 2.4 Upstream failure modes

- [ ] 2.4.1 `test_upstream_eof_mid_sse_finalizes_partial` — upstream closes pipe after 50 SSE events (before message_stop); assert partial accumulated text stored and `ResponseComplete` WsEvent fired
- [ ] 2.4.2 `test_upstream_immediate_eof_no_panic` — upstream sends nothing and closes immediately; session ends cleanly, no panic
- [ ] 2.4.3 `test_client_disconnect_mid_response_still_stores` — client pipe dropped while SSE streaming; u2c keeps running, finalize_response stores content
- [ ] 2.4.4 `test_upstream_idle_timeout_fires` — upstream sends nothing for >120s (use `tokio::time::pause()`); assert WARN timeout fires and session ends

### 2.5 SSE streaming correctness

- [ ] 2.5.1 `test_anthropic_message_stop_terminates_stream` — stream ends on `message_stop` event; response stored, `ResponseComplete` fired
- [ ] 2.5.2 `test_openai_done_sentinel_terminates_stream` — stream ends on `data: [DONE]`; same assertions
- [ ] 2.5.3 `test_tokens_extracted_and_stored` — `message_start` event contains `input_tokens: 100, output_tokens: 50`; stored response message has `tokens_in=100, tokens_out=50`
- [ ] 2.5.4 `test_sse_accumulation_buffer_cap` — SSE stream produces >10 MB text; after cap hit, forwarding continues but accumulated string stops growing; no OOM
- [ ] 2.5.5 `test_sse_split_across_tiny_chunks` — feed SSE response 1 byte at a time; same events extracted as feeding all at once
- [ ] 2.5.6 `test_ws_text_delta_events_fired` — subscribe to `ws_tx` channel; assert one `TextDelta` WsEvent per non-empty SSE delta
- [ ] 2.5.7 `test_response_complete_ws_event_fired` — subscribe to `ws_tx`; assert exactly one `ResponseComplete` after `message_stop`

### 2.6 Storage integration

- [ ] 2.6.1 `test_new_conversation_created_on_first_request` — first request with unknown fingerprint → exactly one `Conversation` file created
- [ ] 2.6.2 `test_conversation_start_ws_event_for_new_conv` — `ws_tx` receives `ConversationStart` for first request, not for second
- [ ] 2.6.3 `test_response_stored_after_sse_complete` — after `message_stop`, call `store.get_messages(conv_id)`; assert one response message with correct content
- [ ] 2.6.4 `test_unparseable_request_no_storage_no_panic` — send garbage body (not JSON); no conversation created, no panic, response still forwarded

## 3. Storage Tests (`src/storage/mod.rs`)

### 3.1 Basic CRUD

- [ ] 3.1.1 `test_insert_and_get_conversation` — insert → `find_conversation_by_fingerprint` returns same conv_id
- [ ] 3.1.2 `test_insert_and_get_messages_in_order` — insert 10 messages → `get_messages` returns all 10 in insertion order
- [ ] 3.1.3 `test_batch_insert_preserves_order` — 100-message batch → read back in same order
- [ ] 3.1.4 `test_list_conversations_newest_first` — insert 15 conversations → `list_conversations` returns exactly 10, newest first

### 3.2 Fingerprinting

- [ ] 3.2.1 `test_same_fingerprint_same_conv` — insert conv with fingerprint A, look up fingerprint A → same conv_id returned
- [ ] 3.2.2 `test_different_provider_same_fingerprint_separate_convs` — fingerprint A for provider "anthropic" and "openai" → two different conv_ids
- [ ] 3.2.3 `test_unknown_fingerprint_returns_none`
- [ ] 3.2.4 `test_count_request_messages_counts_only_requests` — insert 5 request + 3 response messages; `count_request_messages` returns 5

### 3.3 Concurrency

- [ ] 3.3.1 `test_concurrent_batch_inserts_no_corruption` — 10 async tasks each appending 20 messages to the same conv simultaneously; read back and assert total == 200, all lines valid JSON
- [ ] 3.3.2 `test_write_lock_serializes_appends` — assert no interleaved JSON lines under high concurrency (each line must be a complete, parseable Message)

### 3.4 Robustness

- [ ] 3.4.1 `test_malformed_message_line_skipped` — manually write a corrupt JSON line into a conv file; `get_messages` returns remaining valid messages without error
- [ ] 3.4.2 `test_batch_insert_missing_conv_is_noop` — call `batch_insert_messages` for a conv_id with no file; no panic, returns Ok

## 4. Parser Tests (`src/parser/`)

### 4.1 Anthropic request parser (`src/parser/anthropic.rs`)

- [ ] 4.1.1 `test_anthropic_parse_model_and_messages` — JSON with `model`, `messages` array → correct model string and message count
- [ ] 4.1.2 `test_anthropic_parse_tool_use_content_array` — message with `content` as array of blocks → concatenated string
- [ ] 4.1.3 `test_anthropic_parse_image_block_elided` — image block in content → placeholder text, not raw bytes
- [ ] 4.1.4 `test_anthropic_parse_malformed_json_returns_none`
- [ ] 4.1.5 `test_anthropic_parse_missing_messages_returns_none`

### 4.2 Anthropic SSE delta extraction

- [ ] 4.2.1 `test_extract_delta_from_content_block_delta` — `content_block_delta` event with `text_delta` → correct string returned
- [ ] 4.2.2 `test_extract_tokens_from_message_start` — `message_start` event with `usage` block → `(Some(100), None)`
- [ ] 4.2.3 `test_extract_tokens_from_message_delta` — `message_delta` with output tokens → `(None, Some(50))`
- [ ] 4.2.4 `test_empty_delta_returns_none`
- [ ] 4.2.5 `test_non_delta_event_returns_none`

### 4.3 OpenAI parser

- [ ] 4.3.1 `test_openai_parse_model_and_messages`
- [ ] 4.3.2 `test_openai_extract_delta_from_choices`
- [ ] 4.3.3 `test_openai_done_sentinel_is_none` — `data: [DONE]` → `extract_sse_delta` returns None (handled by `is_done_sentinel`)

### 4.4 Google parser

- [ ] 4.4.1 `test_google_parse_contents_field`
- [ ] 4.4.2 `test_google_extract_delta_from_candidates`

### 4.5 Cross-provider

- [ ] 4.5.1 `test_unknown_provider_falls_back_to_openai_format`
- [ ] 4.5.2 `test_parse_request_scaling_182_turns_under_50ms` — performance guard for real-world load

## 5. SSE Parser Tests (`src/parser/sse.rs`)

(Extends existing 6 tests)

- [ ] 5.1 `test_lf_only_line_endings` — `\n` delimiters without `\r` still emit events
- [ ] 5.2 `test_comment_lines_ignored` — lines starting with `:` produce no event
- [ ] 5.3 `test_multi_line_data_concatenated` — two `data:` lines in one event joined with newline
- [ ] 5.4 `test_event_type_without_data_skipped`
- [ ] 5.5 `test_1mb_data_line_no_panic`
- [ ] 5.6 `test_1000_events_in_one_push` — single `push()` call with 1000 events; all returned
- [ ] 5.7 `test_byte_by_byte_same_as_full_push` — feed same SSE bytes one at a time; assert same event sequence as single push
- [ ] 5.8 `test_message_stop_event_parsed_correctly` — Anthropic `event: message_stop\ndata: {...}\n\n`; `event_type == Some("message_stop")`

## 6. Network Helper Tests (`src/proxy/network.rs`)

- [ ] 6.1 `test_peek_sni_extracts_hostname` — use a real TLS ClientHello fixture (captured bytes); assert correct SNI
- [ ] 6.2 `test_peek_sni_returns_none_for_garbage`
- [ ] 6.3 `test_peek_sni_returns_none_for_truncated_buffer`
- [ ] 6.4 `test_intercept_decision_known_hosts` — `api.anthropic.com`, `api.openai.com`, `generativelanguage.googleapis.com` all return `intercept=true`
- [ ] 6.5 `test_intercept_decision_unknown_host` — `example.com` returns `intercept=false`
- [ ] 6.6 `test_dns_query_packet_format` — `build_dns_a_query("api.anthropic.com")` produces a parseable DNS A query packet

## 7. Observability Tests (`src/util.rs`)

- [ ] 7.1 `test_authorization_header_redacted` — `fmt_headers` with `Authorization: Bearer sk-abc123` → value replaced, key preserved
- [ ] 7.2 `test_x_api_key_header_redacted` — `X-Api-Key: abc123` → value replaced
- [ ] 7.3 `test_other_headers_not_redacted` — `Content-Type: application/json` → passed through unchanged
- [ ] 7.4 `test_fmt_chunk_hex_truncates_at_256_bytes` — 1024-byte input → output contains at most 256 hex-encoded bytes
- [ ] 7.5 `test_fmt_chunk_hex_short_input_not_truncated` — 10-byte input → full hex, no truncation marker needed
- [ ] 7.6 `test_fmt_chunk_hex_empty_input` — empty slice → empty or defined sentinel, no panic
