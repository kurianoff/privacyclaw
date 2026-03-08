# Change: Add Structured Three-Level Async Logging

## Why

When issues occur in claudovka (dropped messages, TLS failures, storage anomalies, stalled connections), the current sparse `tracing` calls make it impossible to pinpoint the exact code path or data state. A three-tier structured logging strategy gives full observability at every granularity: DEBUG for every branch + raw transmitted bytes, INFO for every atomic operation, WARN for every lifecycle event — without any functional changes.

## What Changes

- **main.rs**: WARN on each startup mode (CONNECT/network/network-start), graceful shutdown signal; INFO on store open and cert cache init
- **proxy/mod.rs**: WARN on listener bound; INFO on connection accepted; INFO on connection task exit with error
- **proxy/connect.rs**: WARN on MITM established and passthrough established; INFO on CONNECT parsed, intercept decision, TLS handshake completion; DEBUG on each header line drained, raw CONNECT bytes, passthrough/MITM branch reason
- **proxy/intercept.rs**: WARN on session start and session end (conv_id + direction); INFO on each forwarded chunk (bytes), full request body received, response finalized; DEBUG on every loop iteration (read n bytes, write n bytes), raw chunk preview (truncated to 256 bytes), header parsing progress, body accumulation, SSE event fields
- **proxy/network.rs**: WARN on listener bound, connection received (peer_addr), intercept/passthrough decision; INFO on SNI extracted, DNS resolution result, upstream TCP connected, TLS handshakes done; DEBUG on SNI byte-scan steps, DNS query bytes, DNS response parsed
- **storage/mod.rs**: INFO on every public API call with result (insert_conversation, insert_message, batch_insert_messages, find_conversation, count_request_messages, list_conversations, rotate_old); DEBUG on file path resolved, file open/append, bytes written, lines scanned
- **dashboard/mod.rs**: WARN on listener bound, WS client connected, WS client disconnected; INFO on HTTP request received (method path → status bytes), WS event broadcast; DEBUG on request line, header map, path matched
- **ca/mod.rs**: WARN on CA generated, CA installed; INFO on CA loaded; DEBUG on paths checked, DER bytes
- **ca/cert_gen.rs**: INFO on cert cache hit/miss, cert generated for host; DEBUG on cert generation params, cache size
- **parser/mod.rs**, **parser/sse.rs**, **parser/anthropic.rs**, **parser/openai.rs**, **parser/google.rs**: INFO on parsed request (provider, model, N messages, N bytes); DEBUG on each SSE event type, delta length, parse attempt result, JSON field extraction

### Data logging policy (DEBUG)

- Raw transmitted bytes: logged as `chunk_hex` field, truncated at 256 bytes, with `chunk_total_bytes` field showing full size
- HTTP `Authorization` headers: value replaced with `"[REDACTED]"` in structured fields
- All other headers logged verbatim at DEBUG

## Impact

- Affected specs: `mitm-proxy`, `dashboard`, `storage`, `ca-management`, `cli`
- New specs: `observability`
- Affected code: all `.rs` files in `src/`
- No functional changes — pure additive tracing instrumentation
- Zero overhead when log level ≥ INFO (tracing macros are no-ops when level disabled)
