## ADDED Requirements

### Requirement: PII Vault Bidirectional Mapping Correctness
The `PiiVault` unit tests SHALL verify that forward and reverse mappings are mutually consistent, that the Aho-Corasick automaton applies leftmost-longest matching, and that the seeded RNG produces deterministic synthetics.

#### Scenario: Round-trip replace returns original
- **WHEN** a mapping `(original, synthetic)` is added to a vault
- **AND** `replace_originals(original)` is called followed by `replace_synthetics(result)`
- **THEN** the final string equals `original`

#### Scenario: Same conv_id yields same synthetic
- **WHEN** two `PiiVault` instances are created with the same `conversation_id`
- **AND** `get_or_create` is called on both with the same original string and PiiType
- **THEN** both return the identical synthetic string

#### Scenario: Different conv_ids yield different synthetics
- **WHEN** two vaults have different `conversation_id` values
- **AND** `get_or_create` is called with the same original string
- **THEN** the synthetics differ with probability ≥ 0.999 (statistically guaranteed by seeded SHA-256 derivation)

#### Scenario: Leftmost-longest match wins
- **WHEN** the vault contains mappings for both `"Alice"` and `"Alice Brown"`
- **AND** `replace_synthetics` is called on text containing `"Alice Brown"`
- **THEN** exactly one replacement occurs matching `"Alice Brown"`, not two replacements for `"Alice"` and `" Brown"` separately

#### Scenario: Bulk mappings all retrievable
- **WHEN** 20 distinct mappings are added sequentially
- **THEN** all 20 synthetic values are retrievable via `get_synthetic` after each rebuild of the automaton

#### Scenario: VaultRegistry TTL eviction
- **WHEN** a vault was last accessed more than `ttl` ago
- **AND** `evict_expired()` is called
- **THEN** that vault is removed from the registry and a subsequent `get_or_create` for the same conv_id returns a fresh empty vault

---

### Requirement: Tier 1 Regex Entity Coverage
The Tier 1 detector unit tests SHALL include at least one positive and one negative case for every entity type defined in `PiiType`, confirming detection boundaries and Luhn validation.

#### Scenario: Valid email detected
- **WHEN** text contains `user@example.com`
- **THEN** exactly one `PiiSpan` of type `Email` is returned with `confidence = 1.0` and `tier = 1`

#### Scenario: Malformed address not detected
- **WHEN** text contains `not-an-email`
- **THEN** no `PiiSpan` of type `Email` is returned

#### Scenario: Valid Luhn credit card detected
- **WHEN** text contains a 16-digit number passing the Luhn algorithm
- **THEN** a `CreditCard` span is returned

#### Scenario: Invalid Luhn credit card not detected
- **WHEN** text contains a 16-digit number failing the Luhn algorithm
- **THEN** no `CreditCard` span is returned

#### Scenario: API key prefix pattern preserved
- **WHEN** text contains an OpenAI key beginning `sk-proj-` followed by ≥ 40 alphanumeric characters
- **THEN** an `ApiKey` span covering the full key is returned

#### Scenario: Detection span index maps to correct message
- **WHEN** `detect_in_json_messages` is called on a 5-message array where PII appears only in message index 3
- **THEN** the returned `(message_index, spans)` pair has `message_index == 3`

#### Scenario: Email inside URL produces one span
- **WHEN** text contains `https://user@example.com/path`
- **THEN** exactly one `Email` span is returned, not a duplicate

---

### Requirement: Synthetic Data Generator Determinism and Format Correctness
The synthetic generator unit tests SHALL verify that outputs are deterministic for a given seed, structurally appropriate for each PII type, and idempotent across multiple calls for the same original.

#### Scenario: Same seed same output
- **WHEN** `generate()` is called twice with identically seeded `SmallRng` and the same original and PiiType
- **THEN** both calls return the identical synthetic string

#### Scenario: Generated email matches format
- **WHEN** `generate(PiiType::Email, ...)` is called
- **THEN** the output matches the pattern `^[^@]+@example\.com$`

#### Scenario: Generated IPv4 is RFC 1918
- **WHEN** `generate(PiiType::IpV4, ...)` is called
- **THEN** the output begins with `10.` (RFC 1918 private range)

#### Scenario: Generated credit card passes Luhn
- **WHEN** `generate(PiiType::CreditCard, ...)` is called
- **THEN** the generated number satisfies the Luhn algorithm

#### Scenario: API key generation preserves prefix and length
- **WHEN** `generate(PiiType::ApiKey, "sk-proj-abcdef...")` is called
- **THEN** the synthetic value begins with `sk-` and has the same character count as the original

#### Scenario: get_or_create is idempotent
- **WHEN** `get_or_create(vault, original, type)` is called twice for the same original
- **THEN** both calls return the same synthetic string and only one mapping exists in the vault

---

### Requirement: ReplacementBuffer Streaming Correctness
The `ReplacementBuffer` unit tests SHALL verify correct behaviour across all split patterns: mid-token chunk boundary, no match at tail, match at stream end, and zero-vault passthrough.

#### Scenario: Empty vault returns input immediately
- **WHEN** the vault contains zero mappings
- **AND** `process_delta` is called with any input text
- **THEN** the full input is returned immediately with nothing held back

#### Scenario: Match spanning two chunks
- **WHEN** a synthetic token `"ALICE_BROWN_SYNTHETIC"` is split across two `process_delta` calls
- **THEN** the first call returns the safe prefix (bytes before the split point)
- **AND** the second call returns the remaining text with the full synthetic replaced by the original

#### Scenario: No trigger char at tail flushes fully
- **WHEN** the tail of the buffer does not begin with any first character of a synthetic key
- **THEN** all buffered bytes are flushed and none are held back

#### Scenario: flush_remaining empties buffer
- **WHEN** `flush_remaining` is called after a partial stream
- **THEN** all remaining bytes are returned and `self.buffer` is empty

#### Scenario: Throughput with no vault entries
- **WHEN** 1 MB of plain text is processed through `process_delta` in 4 KB increments with an empty vault
- **THEN** the total elapsed time is under 5ms

---

### Requirement: PII Pipeline End-to-End Request Processing
The pipeline unit tests SHALL verify that outbound request bodies are correctly sanitised, that Content-Length is updated when body size changes, that PII mode off is a strict passthrough, and that logs never contain raw PII text.

#### Scenario: No PII body is byte-identical
- **WHEN** `PiiPipeline::process_request` is called on a request body containing no detectable PII
- **THEN** the returned bytes are byte-identical to the input
- **AND** the vault gains zero new mappings

#### Scenario: Single email replaced in output
- **WHEN** the request body contains `"content": "My email is user@example.com"`
- **THEN** the returned bytes contain a synthetic value in place of `user@example.com`
- **AND** the string `user@example.com` is absent from the returned bytes

#### Scenario: Multi-turn history fully scanned
- **WHEN** a 5-turn request has PII in turns 2 and 4
- **THEN** both turns are sanitised in the returned body
- **AND** the vault contains entries for each unique original

#### Scenario: Content-Length updated on size change
- **WHEN** PII replacement changes the body byte length
- **THEN** the `Content-Length` header value in the returned bytes equals `body.len()`

#### Scenario: PII mode off bypasses detection
- **WHEN** `PiiConfig { enabled: false }`
- **THEN** `process_request` returns input bytes unchanged without invoking Tier 1 or Tier 2 detection

#### Scenario: Log output does not contain raw PII
- **WHEN** a request with `user@example.com` is processed
- **AND** tracing output is captured during `PiiPipeline::log_detections`
- **THEN** the captured log lines do not contain `user@example.com`
- **AND** do contain the masked representation `***Email***` or equivalent

---

### Requirement: Vault Persistence Round-Trip
The storage unit tests SHALL verify that a vault can be serialised to the conversation NDJSON file and deserialised back with full mapping fidelity, and that re-saving updates the vault line without creating duplicates.

#### Scenario: Save appends vault line
- **WHEN** `Store::save_vault(conv_id, vault)` is called for a conversation with existing message lines
- **THEN** the NDJSON file gains exactly one line with `"type":"vault"`

#### Scenario: Load restores mappings
- **WHEN** a vault is saved and `Store::load_vault(conv_id)` is called on the same store
- **THEN** the loaded vault has the same `original_to_synthetic` and `synthetic_to_original` entries as the original

#### Scenario: Re-save does not duplicate vault line
- **WHEN** `save_vault` is called twice for the same conv_id
- **THEN** the NDJSON file contains exactly one `"type":"vault"` line (second call overwrites first)

#### Scenario: Missing conversation returns None
- **WHEN** `load_vault` is called for a conv_id with no corresponding file
- **THEN** `Ok(None)` is returned

#### Scenario: Message lines do not confuse vault reader
- **WHEN** the NDJSON file contains multiple message lines followed by a vault line
- **THEN** `load_vault` correctly identifies and deserialises only the vault line

---

### Requirement: Proxy Pipeline PII Integration
The proxy-level integration tests (using `intercept::run` with duplex streams) SHALL verify that the full outbound sanitisation and inbound reversal pipeline satisfies all three correctness invariants end-to-end.

#### Scenario: Original PII absent from upstream request
- **WHEN** the client sends a request containing a known email address
- **AND** `intercept::run` is called with PII mode enabled
- **THEN** the bytes received by the upstream mock do not contain the original email
- **AND** a synthetic token is present in its place

#### Scenario: Original PII restored in client SSE response
- **WHEN** the upstream mock returns an SSE stream containing the synthetic token
- **AND** the client reads the full response
- **THEN** the SSE text received by the client contains the original email
- **AND** the synthetic token is absent from the client-side text

#### Scenario: Content-Length correct after replacement
- **WHEN** PII replacement changes the request body length
- **AND** the upstream mock calls `read_exact(content_length_header_value)`
- **THEN** `read_exact` completes without error (no truncation, no over-read)

#### Scenario: PiiDetected WebSocket event fired
- **WHEN** PII is detected in the outbound request
- **THEN** a `WsEvent::PiiDetected` event is broadcast before `WsEvent::ResponseComplete`
- **AND** `WsEvent::ResponseComplete` is still fired after the SSE stream ends

#### Scenario: PII mode off is byte-identical passthrough
- **WHEN** `PiiConfig { enabled: false }` is passed to `intercept::run`
- **THEN** the bytes forwarded to the upstream are identical to the bytes sent by the client

---

### Requirement: Full Round-Trip Integration
The `tests/integration/pii_roundtrip_test.rs` file SHALL exercise all three correctness invariants together in a single test, using the real `intercept::run`, a real `Store` in a tempdir, and duplex streams for client and upstream.

#### Scenario: Zero-PII leaves machine, round-trip fidelity, idempotency
- **WHEN** the client sends `"Hi, I'm John Smith, john@acme.com"` in a 1-turn request
- **AND** the upstream mock reads the forwarded request
- **THEN** the forwarded request body does not contain `john@acme.com` or `John Smith`
- **WHEN** the upstream mock responds with an SSE stream containing the synthetic names
- **THEN** the client-side SSE text contains `john@acme.com` and `John Smith`
- **AND** no synthetic tokens appear in the client-side text
- **AND** the vault persisted to the tempdir contains the mapping `john@acme.com → <synthetic>`

---

### Requirement: Multi-Turn Mapping Idempotency
The `tests/integration/multiturn_consistency_test.rs` file SHALL verify that the same original PII string produces the same synthetic token across all turns of a conversation.

#### Scenario: PII in turns 1 and 3 maps to same synthetic
- **WHEN** a 5-turn conversation sends `"foo@bar.com"` in turn 1 and again in turn 3
- **THEN** the upstream request for turn 1 and turn 3 contain the same synthetic token
- **AND** the vault entry count for `Email` is exactly 1 (idempotent get_or_create)

---

### Requirement: Vault Persistence Across Proxy Restart
The `tests/integration/vault_persistence_test.rs` file SHALL verify that vault mappings created in a first proxy session are reloaded and reused in a second session opened on the same store directory.

#### Scenario: Turn 2 reuses turn 1 synthetic after restart
- **WHEN** a first `intercept::run` session processes turn 1 and saves the vault to tempdir
- **AND** a new `VaultRegistry` and `Store` are constructed pointing at the same tempdir
- **AND** a second `intercept::run` session processes turn 2 with the same conv_id
- **THEN** the synthetic token in the turn 2 upstream request matches the synthetic from turn 1
- **AND** no new Tier 1 detection is needed for the already-known original

---

### Requirement: Zero-PII Passthrough Leaves No Trace
The `tests/integration/passthrough_no_pii_test.rs` file SHALL verify that a request containing no detectable PII is forwarded byte-identically and leaves no vault artefacts.

#### Scenario: No vault line written for clean request
- **WHEN** a request body contains only generic text with no detectable PII
- **THEN** the forwarded bytes equal the input bytes
- **AND** no `"type":"vault"` line exists in the conversation NDJSON file
- **AND** no `WsEvent::PiiDetected` event is broadcast

---

### Requirement: PII Detection Performance Budget
Performance tests SHALL assert that Tier 1 detection and the ReplacementBuffer operate within the latency budgets defined in the `add-pii-protection` design.

#### Scenario: Tier 1 under 2ms on 10 KB
- **WHEN** `Tier1Detector::detect` is called on a 10 KB text containing 20 PII spans
- **THEN** the call completes in under 2ms

#### Scenario: ReplacementBuffer under 5ms per 1 MB stream
- **WHEN** 1 MB of SSE text containing 50 synthetic tokens is processed through `ReplacementBuffer::process_delta` in 4 KB increments
- **THEN** the total wall-clock time is under 5ms

#### Scenario: Pipeline under 100ms for 182-turn request
- **WHEN** `PiiPipeline::process_request` is called on a 182-turn request body with Tier 1 only enabled
- **THEN** the call completes in under 100ms
