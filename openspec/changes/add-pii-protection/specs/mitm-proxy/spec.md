## MODIFIED Requirements

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
