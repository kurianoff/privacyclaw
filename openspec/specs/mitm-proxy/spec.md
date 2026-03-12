# mitm-proxy Specification

## Purpose
TBD - created by archiving change add-kladovka-mvp. Update Purpose after archive.
## Requirements
### Requirement: HTTP CONNECT Proxy Listener

The proxy SHALL listen on a configurable address (default `127.0.0.1:8080`) and accept HTTP CONNECT tunnel requests from clients.

#### Scenario: CONNECT request accepted

- **WHEN** a client sends `CONNECT api.anthropic.com:443 HTTP/1.1`
- **THEN** the proxy responds with `200 Connection established`
- **AND** proceeds to route based on domain allowlist

#### Scenario: Malformed CONNECT request

- **WHEN** the proxy receives a malformed or non-CONNECT HTTP request
- **THEN** it responds with an appropriate HTTP error and closes the connection gracefully without panicking

### Requirement: Domain-Based Routing

The proxy SHALL route CONNECT requests based on a configurable domain allowlist. Allowlisted domains are MITMed; all others are passed through transparently.

#### Scenario: Allowlisted domain intercepted

- **WHEN** the target domain is in the `[intercept].domains` configuration list
- **THEN** the proxy performs TLS MITM interception

#### Scenario: Non-allowlisted domain passed through

- **WHEN** the target domain is not in the allowlist
- **THEN** the proxy establishes a raw TCP tunnel without TLS termination
- **AND** the original server certificate is presented to the client unchanged

### Requirement: TLS MITM Interception

For allowlisted domains, the proxy SHALL terminate TLS with the client (using a dynamically generated leaf cert) and establish a new TLS connection to the upstream server.

#### Scenario: Full MITM handshake

- **WHEN** intercepting an allowlisted domain
- **THEN** the proxy presents a leaf cert signed by the local CA to the client
- **AND** establishes a TLS connection to the real upstream using webpki-roots for verification
- **AND** forwards decrypted traffic bidirectionally

### Requirement: Zero-Latency Passthrough

The proxy SHALL forward bytes to the client as they arrive from upstream, with parsing occurring on a tee'd copy on a separate channel. The proxy MUST NOT buffer bytes on the critical forwarding path.

#### Scenario: Streaming response forwarding

- **WHEN** an upstream LLM API sends SSE chunks
- **THEN** each chunk is forwarded to the client immediately
- **AND** a copy is sent to the parser channel concurrently without blocking the forward path

### Requirement: Transparent TCP Passthrough

For non-allowlisted domains, the proxy SHALL establish a raw TCP tunnel with no TLS inspection.

#### Scenario: Non-LLM HTTPS passthrough

- **WHEN** a client connects to `github.com:443` via the proxy
- **THEN** a raw TCP tunnel is established to `github.com:443`
- **AND** the client receives the original GitHub TLS certificate
- **AND** no traffic content is inspected or logged

### Requirement: Outbound Request Forwarding

When PII mode is `"replace"` or `"detect-only"`, the proxy SHALL buffer the complete HTTP request body before forwarding to the upstream LLM API. In `"replace"` mode, the proxy SHALL apply the PII pipeline and forward a modified body with an updated `Content-Length` header. In `"detect-only"` mode or when PII mode is `"off"`, the proxy SHALL forward the original bytes with zero additional latency (unchanged Phase 1 behavior).

When PII mode is `"off"`, the outbound path behavior is identical to Phase 1: bytes are forwarded to upstream immediately as they arrive, with no buffering beyond what is needed for logging.

#### Scenario: PII mode off — zero-latency passthrough

- **WHEN** `pii.mode = "off"`
- **THEN** each chunk received from the client is written to upstream immediately
- **AND** no additional buffering is introduced beyond the Phase 1 logging buffer

#### Scenario: PII mode replace — body buffered and rewritten

- **WHEN** `pii.mode = "replace"`
- **AND** a complete request body has been received
- **THEN** the PII pipeline processes the body
- **AND** the modified body plus updated `Content-Length` header are written to upstream as a single write
- **AND** the client's write connection is held open (not closed) during PII processing

#### Scenario: PII mode replace — no PII found

- **WHEN** `pii.mode = "replace"` and the PII pipeline finds zero entities
- **THEN** the original body is forwarded unchanged
- **AND** no vault entry is created for this request

#### Scenario: PII processing error — forward original

- **WHEN** the PII pipeline returns an error (e.g., JSON parse failure, timeout)
- **THEN** the original unmodified body is forwarded to upstream
- **AND** the error is logged at `warn` level
- **AND** the client connection is not dropped

---

### Requirement: Inbound SSE Response Forwarding

When PII mode is `"replace"` and the response is a streaming SSE response, the proxy SHALL apply the `ReplacementBuffer` to each text delta before forwarding to the client. Non-text SSE events (message_start, message_stop, etc.) SHALL be forwarded unchanged. When PII mode is `"off"` or `"detect-only"`, raw bytes are forwarded byte-identical to upstream (Phase 1 behavior).

#### Scenario: Streaming replacement applied

- **WHEN** `pii.mode = "replace"`
- **AND** an SSE `content_block_delta` event arrives with text containing a synthetic token
- **THEN** the proxy extracts the text delta, applies reverse replacement, re-wraps in the SSE event structure, and forwards to the client

#### Scenario: Non-text events forwarded unchanged

- **WHEN** an SSE event with `event: message_start` or `event: message_stop` arrives
- **THEN** the raw event bytes are forwarded to the client without modification

#### Scenario: PII mode off — byte-identical response

- **WHEN** `pii.mode = "off"`
- **THEN** the proxy forwards raw SSE bytes to the client exactly as received from upstream
- **AND** behavior is identical to Phase 1

---

### Requirement: VaultRegistry Injection

The proxy SHALL accept a `VaultRegistry` and `PiiConfig` reference at initialization time and pass them through to each intercepted connection handler. The `VaultRegistry` is a singleton per proxy instance.

#### Scenario: VaultRegistry constructed at startup

- **WHEN** `claudovka start` is invoked with `pii.mode != "off"`
- **THEN** a `VaultRegistry` is constructed and passed to both `proxy::run` and `proxy::network::run`

#### Scenario: VaultRegistry not constructed when PII is off

- **WHEN** `pii.mode = "off"` (default)
- **THEN** no `VaultRegistry` is allocated
- **AND** no PII-related processing occurs in connection handlers

### Requirement: Request and Response Forwarding Fidelity
The proxy SHALL forward request bytes to upstream and response bytes to the client byte-for-byte without modification, regardless of payload size.

#### Scenario: Small request forwarded verbatim
- **WHEN** a client sends a 1-turn Anthropic request (~3 KB)
- **THEN** the upstream receives the exact same bytes the client sent

#### Scenario: Large request forwarded verbatim
- **WHEN** a client sends a 40-turn request (~600 KB)
- **THEN** the upstream receives the exact same bytes, and the TLS write buffer is flushed before the proxy waits for the response

#### Scenario: Response forwarded verbatim
- **WHEN** upstream sends a streaming SSE response
- **THEN** the client receives the exact same bytes upstream sent, with no insertions or deletions

### Requirement: TLS Flush After Complete Request Body
After the proxy has forwarded the last byte of a Content-Length–framed request body to upstream, it SHALL flush the upstream TLS write buffer before awaiting the response.

#### Scenario: Large request body fully delivered to upstream
- **WHEN** a Content-Length request body is fully forwarded across multiple TCP chunks
- **THEN** `writer.flush()` is called before the proxy begins reading the upstream response
- **AND** the upstream receives the complete body and responds normally

#### Scenario: Flush error is propagated
- **WHEN** the flush to upstream fails (e.g., connection reset)
- **THEN** the c2u task returns an error and the session ends cleanly

### Requirement: Keep-Alive Multi-Turn Correctness
The proxy SHALL correctly handle multiple sequential HTTP request/response pairs on a single keep-alive connection, resetting all per-request state between turns.

#### Scenario: Second turn on same connection
- **WHEN** the client sends a second request on the same TLS connection after the first response completes
- **THEN** the second request is forwarded correctly and the second response is received correctly

#### Scenario: Per-request state reset
- **WHEN** the second request has a different Content-Length than the first
- **THEN** `body_received` and `content_length` are reset between turns and the correct body_bytes are logged for each turn independently

### Requirement: Upstream Failure Handling
The proxy SHALL handle upstream failures gracefully: finalizing any partially received response and closing the session cleanly without panic.

#### Scenario: Upstream closes mid-SSE-stream
- **WHEN** the upstream closes the connection after sending 50 SSE events but before `message_stop`
- **THEN** the accumulated partial response text is stored
- **AND** a `ResponseComplete` WebSocket event is fired
- **AND** the session ends without panic

#### Scenario: Upstream sends nothing and closes
- **WHEN** the upstream closes immediately without sending any bytes
- **THEN** u2c exits cleanly, no content is stored, and no panic occurs

#### Scenario: Upstream idle timeout
- **WHEN** the upstream sends no data for 120 seconds
- **THEN** a WARN-level "upstream idle timeout" is logged and the session ends

### Requirement: Client Disconnect Handling
When the client disconnects mid-response, the proxy SHALL continue reading the upstream response and finalize storage before exiting.

#### Scenario: Client drops connection during SSE stream
- **WHEN** the client pipe is closed while the upstream is still streaming SSE events
- **THEN** u2c continues to drain the upstream and calls `finalize_response`
- **AND** the accumulated content is stored

### Requirement: Concurrent Session Isolation
Multiple simultaneous proxy sessions SHALL not share state and SHALL not corrupt each other's storage or response forwarding.

#### Scenario: Two concurrent sessions with different requests
- **WHEN** two sessions are active simultaneously with different conversation fingerprints
- **THEN** each session stores its own conversation and messages independently
- **AND** each client receives only its own upstream response bytes

#### Scenario: Two concurrent sessions with the same fingerprint
- **WHEN** two sessions arrive simultaneously with the same first-message fingerprint
- **THEN** both resolve to the same conv_id
- **AND** storage is not corrupted (no duplicate conversation header, no interleaved JSON lines)

