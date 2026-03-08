## ADDED Requirements

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
