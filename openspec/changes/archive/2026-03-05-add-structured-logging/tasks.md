# Tasks: Add Structured Three-Level Async Logging

## 1. Shared utility

- [x] 1.1 Add `fmt_chunk_hex(data: &[u8], max: usize) -> String` to `src/util.rs` — formats bytes as lowercase hex, truncates at `max` with `"...(N total bytes)"` suffix
- [x] 1.2 Add `fmt_headers(raw_header_block: &str) -> String` to `src/util.rs` — replaces values of `authorization` and `x-api-key` headers with `"[REDACTED]"`

## 2. src/main.rs

- [x] 2.1 WARN: `"Starting claudovka in CONNECT mode"` (before spawning proxy_task in cmd_start)
- [x] 2.2 WARN: `"Starting claudovka in network mode"` (before spawning net_task in cmd_network_start)
- [x] 2.3 WARN: `"Shutting down claudovka"` (in the ctrl_c handler of both cmd_start and cmd_network_start)
- [x] 2.4 INFO: `"Store opened"` with `logs_dir` field after Store::open in both cmd_start and cmd_network_start
- [x] 2.5 INFO: `"Cert cache initialised"` after CertCache::new
- [x] 2.6 WARN: `"Log rotation: deleted N file(s)"` — already exists at INFO; change to WARN
- [x] 2.7 DEBUG: `"rotation_loop: sleeping N seconds until midnight"` at top of rotation_loop

## 3. src/proxy/mod.rs (CONNECT listener)

- [x] 3.1 WARN: `"CONNECT proxy bound"` with `addr` field — change existing `tracing::info!` to WARN
- [x] 3.2 INFO: `"CONNECT: accepted connection"` with `peer_addr` — change existing `tracing::debug!` to INFO
- [x] 3.3 DEBUG: `"CONNECT: connection task finished"` with error context on Err (after the spawn completes/errors logged in the spawn)

## 4. src/proxy/connect.rs

- [x] 4.1 DEBUG: `"connect: reading CONNECT request line"` at start of handle()
- [x] 4.2 DEBUG: `"connect: drained header line"` with truncated `line` field, inside the drain loop
- [x] 4.3 INFO: `"connect: CONNECT parsed"` with `host` and `port` fields after parse_connect succeeds
- [x] 4.4 INFO: `"connect: intercept=true"` or `"connect: intercept=false"` with `host` field at the is_intercepted branch
- [x] 4.5 WARN: `"connect: passthrough established"` with `host` and `port` — inside `passthrough()` after copy_bidirectional returns
- [x] 4.6 WARN: `"connect: MITM session started"` with `host` and `port` — at start of `mitm()`
- [x] 4.7 INFO: `"connect: upstream TCP connected"` with `addr` field inside mitm()
- [x] 4.8 INFO: `"connect: upstream TLS handshake done"` inside mitm()
- [x] 4.9 INFO: `"connect: client TLS handshake done"` inside mitm()
- [x] 4.10 WARN: `"connect: MITM session ended"` with `host` after intercept::run returns in mitm()
- [x] 4.11 DEBUG: `"connect: passthrough: upstream connected"` with `addr` inside passthrough()
- [x] 4.12 DEBUG: `"connect: sending 200 Connection established to client"` inside mitm() and passthrough()

## 5. src/proxy/intercept.rs

- [x] 5.1 WARN: `"intercept: session started"` with `host` and `provider` at start of run()
- [x] 5.2 WARN: `"intercept: session ended"` with `host` after join! in run()
- [x] 5.3 DEBUG: `"c2u: loop iteration start"` with `raw_len`, `header_done`, `body_received` at top of handle_c2u loop
- [x] 5.4 INFO: `"c2u: read chunk"` with `bytes=n`, `total_raw=raw.len()` after each successful read
- [x] 5.5 DEBUG: `"c2u: chunk data"` with `chunk_hex=fmt_chunk_hex(chunk,256)` and `total_bytes=n`
- [x] 5.6 INFO: `"c2u: forwarded chunk to upstream"` with `bytes=n` after write_all
- [x] 5.7 DEBUG: `"c2u: header delimiter found"` with `body_start`, `content_length` when find_header_end succeeds
- [x] 5.8 DEBUG: `"c2u: HTTP headers"` with `headers=fmt_headers(headers_text)` (using first hdr_end bytes)
- [x] 5.9 INFO: `"c2u: full request body received"` with `body_bytes=len` when body.len() >= content_length
- [x] 5.10 DEBUG: `"c2u: resetting per-request state"` with `body_received` when state resets
- [x] 5.11 DEBUG: `"c2u: chunked/unknown-length body end"` with `body_bytes` at stream EOF fallback path
- [x] 5.12 DEBUG: `"u2c: loop iteration start"` with `chunk_count`, `header_done`, `is_sse`, `body_received` at top of handle_u2c loop
- [x] 5.13 INFO: `"u2c: read chunk from upstream"` with `bytes=n`, `chunk=chunk_count`
- [x] 5.14 DEBUG: `"u2c: chunk data"` with `chunk_hex=fmt_chunk_hex(chunk,256)` and `total_bytes=n`
- [x] 5.15 INFO: `"u2c: forwarded chunk to client"` with `bytes=n`, `write_ms`
- [x] 5.16 DEBUG: `"u2c: response headers parsed"` with `is_sse`, `content_length` when header_done flips true
- [x] 5.17 DEBUG: `"u2c: response HTTP headers"` with `headers=fmt_headers(headers_text)`
- [x] 5.18 DEBUG: `"u2c: SSE event"` with `event_type`, `data_len` for every SSE event in process_response_chunk
- [x] 5.19 DEBUG: `"u2c: SSE data payload"` with `data=event.data` (truncated at 256 chars) for every SSE event
- [x] 5.20 INFO: `"u2c: response complete (SSE [DONE])"` or `"u2c: response complete (Content-Length)"` with `body_received`, `is_sse`
- [x] 5.21 DEBUG: `"u2c: resetting per-response state"` with `accumulated_len`, `chunk_count` at state reset
- [x] 5.22 DEBUG: `"finalize_response: called"` with `conv_id_present`, `accumulated_len`, `body_buf_len`
- [x] 5.23 INFO: `"finalize_response: stored response"` with `conv_id`, `content_len`, `tokens_in`, `tokens_out`

## 6. src/proxy/network.rs

- [x] 6.1 WARN: `"network proxy bound"` with `addr` — change existing `tracing::info!` to WARN
- [x] 6.2 WARN: `"network: accepted connection"` with `peer_addr` — change existing `tracing::debug!` to WARN
- [x] 6.3 INFO: `"network: SNI extracted"` with `host` after peek_sni succeeds
- [x] 6.4 INFO: `"network: intercept=true"` or `"network: intercept=false"` with `host` at is_intercepted branch
- [x] 6.5 WARN: `"network: intercepting host"` — change existing `tracing::debug!` to WARN
- [x] 6.6 INFO: `"network: client TLS handshake done"` after acceptor.accept
- [x] 6.7 INFO: `"network: DNS resolved"` with `host`, `ip` — change existing `tracing::debug!` to INFO
- [x] 6.8 INFO: `"network: upstream TCP connected"` with `host`, `ip`, `port=443`
- [x] 6.9 INFO: `"network: upstream TLS handshake done"` with `host`
- [x] 6.10 WARN: `"network: passthrough established"` with `host`, `ip` inside passthrough_raw() — change existing debug to WARN
- [x] 6.11 DEBUG: `"network: peek buf"` with `peek_hex=fmt_chunk_hex(&peek_buf[..n], 64)`, `n` at start of handle()
- [x] 6.12 DEBUG: `"network: SNI parse: no SNI"` — change existing `tracing::debug!` to DEBUG with `peek_bytes=n`
- [x] 6.13 DEBUG: `"network: resolve_bypass_hosts: querying DNS"` with `hostname`, `dns_server` inside resolve_bypass_hosts
- [x] 6.14 DEBUG: `"network: resolve_bypass_hosts: got A record"` with `hostname`, `ip` on success
- [x] 6.15 DEBUG: `"network: passthrough_raw: loopback drop"` — change existing debug to keep as DEBUG with `host`, `ip`
- [x] 6.16 WARN: `"network: connection handler error"` with `peer_addr`, `err` — change existing `tracing::warn!`

## 7. src/storage/mod.rs

- [x] 7.1 INFO: `"storage: insert_conversation"` with `conv_id`, `provider`, `model` at start of insert_conversation
- [x] 7.2 INFO: `"storage: insert_conversation ok"` with `conv_id`, `path` after write succeeds
- [x] 7.3 INFO: `"storage: batch_insert_messages"` with `conv_id`, `count=messages.len()` at start of batch_insert_messages
- [x] 7.4 DEBUG: `"storage: resolved conv file path"` with `conv_id`, `path` after conv_file_path
- [x] 7.5 DEBUG: `"storage: batch_insert_messages: serialised"` with `buf_bytes=buf.len()` before acquiring lock
- [x] 7.6 INFO: `"storage: batch_insert_messages ok"` with `conv_id`, `count`, `bytes_written` after write
- [x] 7.7 INFO: `"storage: find_conversation_by_fingerprint"` with `provider`, `fingerprint_prefix` (first 8 chars) at entry
- [x] 7.8 INFO: `"storage: find_conversation_by_fingerprint: found"` with `conv_id` on success
- [x] 7.9 INFO: `"storage: find_conversation_by_fingerprint: not found"` on miss
- [x] 7.10 INFO: `"storage: count_request_messages"` with `conv_id`, `count` (result) after scan
- [x] 7.11 INFO: `"storage: list_conversations"` with `total_files`, `returned` after listing
- [x] 7.12 INFO: `"storage: rotate_old"` with `deleted` on completion
- [x] 7.13 DEBUG: `"storage: conv_file_path scan"` with `logs_dir`, `suffix` at entry to conv_file_path

## 8. src/dashboard/mod.rs

- [x] 8.1 WARN: `"dashboard bound"` with `addr` — change existing `tracing::info!` to WARN
- [x] 8.2 WARN: `"dashboard: WebSocket client connected"` with `peer` in handle_ws_upgrade
- [x] 8.3 WARN: `"dashboard: WebSocket client disconnected"` with `peer` when ws loop exits
- [x] 8.4 INFO: `"dashboard: HTTP request"` with `method`, `path` in handle_http at entry
- [x] 8.5 INFO: `"dashboard: HTTP response"` with `path`, `status`, `body_bytes` before writing response
- [x] 8.6 INFO: `"dashboard: WS event sent"` with `event_type` when sink.send succeeds
- [x] 8.7 DEBUG: `"dashboard: request line"` with `request_line` in dispatch()
- [x] 8.8 DEBUG: `"dashboard: headers"` with `header_count=headers.len()` in dispatch()
- [x] 8.9 DEBUG: `"dashboard: routing"` with `path`, `is_ws_upgrade` before dispatch branch

## 9. src/ca/mod.rs

- [x] 9.1 WARN: `"ca: CA generated"` with `cert_path` — change existing `tracing::info!` to WARN
- [x] 9.2 WARN: `"ca: CA installed into OS trust store"` — change existing `tracing::info!` to WARN
- [x] 9.3 INFO: `"ca: CA loaded from disk"` with `cert_path` in load_ca on success
- [x] 9.4 INFO: `"ca: CA not found, returning None"` in load_ca when files absent
- [x] 9.5 INFO: `"ca: CA deleted"` with `ca_dir` in delete_ca
- [x] 9.6 DEBUG: `"ca: checking paths"` with `cert_path`, `key_path` in load_ca
- [x] 9.7 DEBUG: `"ca: cert DER bytes"` with `der_bytes=cert_der.len()` in load_ca after pem_cert_to_der
- [x] 9.8 DEBUG: `"ca: generating key pair"` in generate_ca
- [x] 9.9 DEBUG: `"ca: writing cert and key"` with `cert_path`, `key_path` in generate_ca

## 10. src/ca/cert_gen.rs

- [x] 10.1 INFO: `"cert_gen: cache hit"` with `host` in CertCache::get_or_create on cache hit
- [x] 10.2 INFO: `"cert_gen: cache miss, generating cert"` with `host` on cache miss
- [x] 10.3 INFO: `"cert_gen: cert generated"` with `host` after successful cert build
- [x] 10.4 DEBUG: `"cert_gen: cache size"` with `size=cache.len()` on every lookup
- [x] 10.5 DEBUG: `"cert_gen: building CertifiedKey"` with `host` inside build_certified_key
- [x] 10.6 DEBUG: `"cert_gen: cert DER size"` with `der_bytes` after cert serialization

## 11. src/parser/sse.rs

- [x] 11.1 DEBUG: `"sse: push called"` with `input_bytes=chunk.len()`, `buf_len` at start of push()
- [x] 11.2 DEBUG: `"sse: event parsed"` with `event_type`, `data_len=event.data.len()` for each event emitted
- [x] 11.3 DEBUG: `"sse: [DONE] sentinel detected"` in is_done_sentinel

## 12. src/parser/anthropic.rs, openai.rs, google.rs, mod.rs

- [x] 12.1 INFO: `"parser: parse_request"` with `provider`, `body_bytes`, `messages=parsed.messages.len()`, `model` on success in parser::parse_request
- [x] 12.2 DEBUG: `"parser: parse_request failed"` with `provider`, `body_bytes`, `err` on None return
- [x] 12.3 DEBUG: `"parser: extract_sse_delta"` with `provider`, `event_type`, `delta_len` in each provider's extract function
- [x] 12.4 DEBUG: `"parser: extract_message_start_tokens"` with `tokens_in`, `tokens_out` in anthropic extract

## 13. Validation and cleanup

- [x] 13.1 Run `cargo build` and fix all compilation errors
- [x] 13.2 Run `cargo clippy -- -D warnings` and fix all warnings
- [x] 13.3 Run `cargo test` and ensure all tests pass
- [ ] 13.4 Manual smoke test: `cargo run -- start` with `RUST_LOG=claudovka=debug` and verify log output at each level
- [ ] 13.5 Manual smoke test: verify INFO output contains no DEBUG-only fields
- [ ] 13.6 Manual smoke test: verify Authorization header value does not appear in DEBUG logs
