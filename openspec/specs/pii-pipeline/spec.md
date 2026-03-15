# pii-pipeline Specification

## Purpose
TBD - created by archiving change add-pii-protection. Update Purpose after archive.
## Requirements
### Requirement: Tiered PII Detection Pipeline

The system SHALL implement a three-tier PII detection pipeline that runs on the fully-buffered outbound request body. All three tiers operate in sequence on the same text. Later tiers SHALL skip spans already detected by earlier tiers. The pipeline SHALL be configurable: Tier 1 is always active when PII mode is enabled; Tiers 2 and 3 are independently opt-in.

#### Scenario: Tier 1 only (default)

- **WHEN** `pii.tiers.regex = true` and `pii.tiers.ner = false`
- **AND** the request body contains `"email john@acme.com, SSN 123-45-6789"`
- **THEN** the email and SSN are detected and replaced within <2ms added latency
- **AND** the modified body is forwarded to the LLM

#### Scenario: Tier 1+2 enabled

- **WHEN** `pii.tiers.ner = true` and GLiNER model is installed
- **AND** the request body contains `"Ask Maria Johnson at 42 Oak Street"`
- **THEN** Tier 1 finds no structured PII
- **AND** Tier 2 (GLiNER) detects `[PERSON: Maria Johnson]` and `[LOCATION: 42 Oak Street]`
- **AND** both are replaced with synthetic equivalents

#### Scenario: Tier 2 timeout fallback

- **WHEN** Tier 2 inference exceeds 500ms
- **THEN** a warning is logged (`tracing::warn`)
- **AND** only Tier 1 results are used for replacement
- **AND** the request is forwarded without further delay

#### Scenario: PII mode off

- **WHEN** `pii.mode = "off"` (the default)
- **THEN** the pipeline is not invoked
- **AND** request bytes are forwarded byte-identical to upstream (Phase 1 behavior)

---

### Requirement: Tier 1 Regex Detection

The Tier 1 detector SHALL use compiled regular expressions ported from Microsoft Presidio to detect structured PII. Patterns SHALL use the `regex` crate where possible and `fancy-regex` for patterns requiring lookahead/lookbehind assertions.

Entity types and their sources:
- `Email` — RFC 5321 simplified pattern
- `Phone` — US/international, with optional country code
- `Ssn` — US SSN `###-##-####` with negative lookahead for invalid prefixes
- `CreditCard` — Visa/MC/Amex/Discover with Luhn checksum validation
- `IpV4`, `IpV6`
- `OpenAiApiKey` — `sk-[A-Za-z0-9]{48}`
- `AwsAccessKey` — `AKIA[0-9A-Z]{16}`
- `AwsSecretKey` — context-aware pattern near `secret` keyword
- `GitHubPat` — `ghp_[A-Za-z0-9]{36}` or `github_pat_[A-Za-z0-9_]{82}`
- `BearerToken` — `Bearer [A-Za-z0-9._~+/-]+=*`
- `SshPrivateKey` — `-----BEGIN ... PRIVATE KEY-----`
- `DbConnectionString` — `(postgres|mysql|mongodb|redis)://[^@]*@`
- `UrlWithCreds` — `https?://[^:]+:[^@]+@`

#### Scenario: Credit card with Luhn validation

- **WHEN** the text contains `"4532015112830366"` (valid Visa, passes Luhn)
- **THEN** the credit card is detected
- **WHEN** the text contains `"4532015112830367"` (invalid Luhn)
- **THEN** it is NOT detected as a credit card

#### Scenario: Bearer token detection

- **WHEN** the text contains `"Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9..."`
- **THEN** the token value after `Bearer ` is detected and replaced
- **AND** the `Bearer ` prefix is preserved in the output

#### Scenario: No false positive on short hex strings

- **WHEN** the text contains a git commit SHA like `"a3f5c2d"` (7 chars)
- **THEN** it is NOT detected as an API key

---

### Requirement: Outbound Request Body Rewriting

When PII mode is `"replace"`, the proxy SHALL buffer the complete HTTP request body before forwarding to the upstream LLM API. After detection and replacement, the modified body SHALL be forwarded with an updated `Content-Length` header. The body JSON structure SHALL be preserved — only the `content` field values within `messages` are modified.

#### Scenario: Content-Length updated after replacement

- **WHEN** replacement changes the request body size (e.g., `"john@acme.com"` → `"alice.brown@example.com"`)
- **THEN** the `Content-Length` header in the forwarded request reflects the new body length
- **AND** the upstream TLS connection is not corrupted

#### Scenario: All message roles processed

- **WHEN** the request body contains messages with roles `system`, `user`, and `assistant` (conversation history)
- **THEN** PII is detected and replaced in all message `content` fields
- **AND** replacements are consistent across roles (same original → same synthetic)

#### Scenario: Detect-only mode

- **WHEN** `pii.mode = "detect-only"`
- **THEN** PII is detected and logged to the dashboard
- **AND** the original body is forwarded to upstream UNCHANGED

#### Scenario: Malformed JSON body

- **WHEN** the request body is not valid JSON
- **THEN** a warning is logged
- **AND** the original body is forwarded unchanged (no crash, no data loss)

---

### Requirement: Synthetic Replacement Generation

For each detected PII entity, the system SHALL generate a synthetic replacement that:
- Preserves the PII type (name → name, email → email)
- Preserves the format and approximate length
- Is deterministic within a conversation (same original → same synthetic for the conversation lifetime)
- Is culturally plausible for the detected locale

#### Scenario: Email replacement format

- **WHEN** `"john.doe@acme.com"` is replaced
- **THEN** the synthetic value follows the pattern `"{first}.{last}@example.com"`
- **AND** the `example.com` domain is always used (never a real domain)

#### Scenario: API key replacement preserves prefix

- **WHEN** `"sk-abc123..."` (OpenAI key) is replaced
- **THEN** the synthetic value starts with `"sk-"` followed by random alphanumeric characters
- **AND** the synthetic key has the same total length as the original

#### Scenario: Person name locale preservation

- **WHEN** the detected name appears to be Russian (Cyrillic or common Russian names)
- **THEN** the synthetic name is also a plausible Russian name
- **WHEN** the detected name appears to be Korean (Hangul)
- **THEN** the synthetic name is a plausible Korean name

---

### Requirement: Streaming Inbound Reverse Replacement

After the LLM responds with a streaming SSE response containing synthetic tokens, the proxy SHALL replace synthetic values back to the originals in real time before forwarding to the client. The replacement SHALL operate on the text delta extracted from each SSE event, not on raw bytes.

#### Scenario: Synthetic token reversed in mid-stream

- **WHEN** the SSE stream delivers `"Alice Brown"` as a synthetic replacement for `"John Smith"`
- **AND** `"Alice"` arrives in one SSE chunk and `" Brown"` in the next
- **THEN** the `ReplacementBuffer` holds `"Alice"` until the potential completion of the token is resolved
- **AND** when `" Brown"` arrives, the buffer recognises `"Alice Brown"`, flushes `"John Smith"` to the client

#### Scenario: Zero-latency for text without synthetic tokens

- **WHEN** an SSE text delta does not begin with any prefix of a synthetic key
- **THEN** the text is flushed immediately without buffering
- **AND** no additional latency is added

#### Scenario: Buffer flushed at stream end

- **WHEN** the SSE stream ends (message_stop or `data: [DONE]`)
- **THEN** all remaining buffered text is flushed to the client
- **AND** `ReplacementBuffer::flush_remaining()` is called

#### Scenario: SSE envelope preserved

- **WHEN** the proxy modifies the text delta inside an SSE event
- **THEN** the SSE event structure (`event:`, `data:`) and all non-text fields are preserved
- **AND** the client receives a syntactically valid SSE event stream

#### Scenario: Empty vault (no PII detected)

- **WHEN** the vault is empty (no PII was found in the request)
- **THEN** the `ReplacementBuffer` passes all text through immediately
- **AND** the behavior is identical to Phase 1 passthrough

---

### Requirement: Locale Pack Support

The system SHALL support configurable locale packs that extend Tier 1 detection with country-specific entity recognizers. Locale packs are TOML files loaded from a configurable directory.

#### Scenario: Indian Aadhaar detection

- **WHEN** locale `in-IN` is active
- **AND** the text contains `"Aadhaar: 1234 5678 9012"`
- **THEN** the 12-digit number is detected as `AadhaarNumber`
- **AND** a synthetic 12-digit replacement is generated

#### Scenario: Brazilian CPF detection

- **WHEN** locale `br-BR` is active
- **AND** the text contains `"CPF: 123.456.789-09"`
- **THEN** it is detected as `CpfNumber`
- **AND** the CPF checksum is validated before accepting the match

#### Scenario: Locale pack not found

- **WHEN** a locale is configured but its pack file is not found in `locale_dir`
- **THEN** a warning is logged
- **AND** only universal (en-US) patterns are used — no crash

---

### Requirement: test-pii CLI Command

The system SHALL provide a `privacyclaw test-pii` command that runs the detection pipeline on user-supplied text and prints a human-readable table of all detected entities, their type, tier, confidence, and proposed synthetic replacement.

#### Scenario: Structured PII detected

- **WHEN** the user runs `privacyclaw test-pii "My email is john@acme.com, SSN 123-45-6789"`
- **THEN** the output lists:
  - `[EMAIL] "john@acme.com" → "alice.brown@example.com" (Tier 1)`
  - `[SSN] "123-45-6789" → "987-65-4321" (Tier 1)`

#### Scenario: No PII detected

- **WHEN** the user runs `privacyclaw test-pii "Hello, how are you?"`
- **THEN** the output states `No PII detected`
- **AND** the exit code is 0

#### Scenario: Locale-specific detection

- **WHEN** the user runs `privacyclaw test-pii --locale in-IN "My Aadhaar is 1234 5678 9012"`
- **THEN** the Aadhaar number is detected and displayed

### Requirement: Tier 1 IPv6 Detection Precision

The Tier 1 IPv6 detector SHALL reject false positives caused by Rust module path separators and other `::` occurrences in source code. A detected IPv6 candidate SHALL only be accepted if it passes the `ipv6_valid` post-match validator.

**Validator rules (all must hold):**
- The match contains at least 2 colon (`:`) characters.
- No colon-separated segment in the match is longer than 4 characters. (Rust identifiers like `vault`, `buffer`, `pii` exceed 4 chars and are therefore rejected.)

The IPv6 regex SHALL use a negative lookbehind `(?<![:\w])` to prevent matching inside larger identifiers.

#### Scenario: Rust path separator not detected as IPv6

- **GIVEN** the proxy is in PII-replace mode
- **WHEN** the user sends a message containing `use crate::pii::vault::PiiVault;`
- **THEN** `Tier1Detector::detect()` returns zero spans with `entity_type == IpV6`
- **AND** the text is forwarded to the LLM unmodified for that span

#### Scenario: Double-colon identifier not detected as IPv6

- **GIVEN** the proxy is in PII-replace mode
- **WHEN** the user sends a message containing `foo::bar` or `MyModule::method`
- **THEN** no IpV6 span is detected

#### Scenario: Abbreviated IPv6 still detected

- **WHEN** the user sends a message containing `fe80::1` or `2001:db8::1` or `fd00::1` or `::1`
- **THEN** exactly one IpV6 span is detected covering the address

#### Scenario: Full 8-group IPv6 still detected

- **WHEN** the user sends `2001:0db8:85a3:0000:0000:8a2e:0370:7334`
- **THEN** exactly one IpV6 span is detected

### Requirement: Tier 1 Detector Tracing

The `Tier1Detector` SHALL emit `tracing` calls at DEBUG and INFO level for detection results and at TRACE level for per-pattern branch decisions.

#### Scenario: DEBUG record per detected span

- **WHEN** `Tier1Detector::detect` (or `find_all`) matches any PII span in the input text
- **THEN** a `tracing::debug!` call is emitted for each span containing fields: `entity_type`, `span_start`, `span_end`, `confidence`, `text_len`
- **AND** the call is made after all patterns for that entity type have run (not once per regex attempt)

#### Scenario: INFO summary after full detection run

- **WHEN** `Tier1Detector::detect` completes processing all 14+ entity patterns against the input text
- **THEN** a `tracing::info!` call is emitted with fields: `entity_type = "tier1"`, `count` (total spans found), `text_len`

#### Scenario: TRACE for each pattern attempt

- **WHEN** `RUST_LOG=trace` is set
- **THEN** for each of the 14+ entity patterns, a `tracing::trace!` call is emitted with the pattern name and whether it matched (boolean field `matched`)
- **AND** for `CreditCard` patterns, an additional `tracing::trace!` call records `entity_type = "CreditCard"`, `span_start`, `span_end`, and `verdict` (`"pass"` or `"fail_luhn"`)

---

### Requirement: Synthetic Generator Tracing

The `SyntheticGenerator` SHALL emit `tracing` calls at DEBUG level for cache hits and at INFO level for first-time synthetic value generation.

#### Scenario: INFO on first synthetic generation

- **WHEN** `SyntheticGenerator::get_or_create` is called for an `(entity_type, original)` pair not previously seen in the current conversation
- **THEN** a `tracing::info!` call is emitted with fields: `conv_id`, `entity_type`, `original`, `synthetic`

#### Scenario: DEBUG on cache hit

- **WHEN** `SyntheticGenerator::get_or_create` is called for an `(entity_type, original)` pair already in the cache
- **THEN** a `tracing::debug!` call is emitted with fields: `conv_id`, `entity_type`, `original_len`
- **AND** `original` and `synthetic` values are NOT included in the DEBUG record (to limit payload exposure at non-INFO levels)

#### Scenario: TRACE on generate internals

- **WHEN** `RUST_LOG=trace` is set and `SyntheticGenerator::generate` is called
- **THEN** a `tracing::trace!` call is emitted on entry with `entity_type` and `input_len`
- **AND** a `tracing::trace!` call is emitted on return with `entity_type` and `synthetic`

---

### Requirement: Replacement Buffer Tracing

The `ReplacementBuffer` SHALL emit `tracing` calls at DEBUG level for buffer state transitions and at TRACE level for per-character match decisions.

#### Scenario: DEBUG record on every process_delta call

- **WHEN** `ReplacementBuffer::process_delta` is called with any input
- **THEN** a `tracing::debug!` call is emitted with fields: `text_len`, `holdback_len` (before processing), `flushed_len` (bytes written to output this call)

#### Scenario: TRACE on synthetic-token boundary match

- **WHEN** `RUST_LOG=trace` is set
- **THEN** for each synthetic token prefix match or abandonment decision in `process_delta`, a `tracing::trace!` call is emitted with fields: `matched_key` (the synthetic token string or prefix), `span_start`, `span_end` (positions within the current delta)

#### Scenario: INFO when flush_remaining produces output

- **WHEN** `ReplacementBuffer::flush_remaining` is called and the holdback buffer is non-empty
- **THEN** a `tracing::info!` call is emitted with field: `flushed_len` (number of bytes flushed)

#### Scenario: No log call when buffer and flush are both empty

- **WHEN** `process_delta` is called with empty input and `flush_remaining` is called with an empty holdback
- **THEN** no tracing macro is invoked (avoiding empty-record noise)

---

### Requirement: PII Vault Tracing

The `PiiVault` SHALL emit `tracing` calls at DEBUG level for lookup results and at INFO/WARN level for structural events (insert, reload, error).

#### Scenario: DEBUG on vault lookup

- **WHEN** `PiiVault::get_synthetic` or `PiiVault::get_original` is called
- **THEN** a `tracing::debug!` call is emitted with fields: `conv_id`, `entity_type`, `hit` (bool)
- **AND** the actual key/value strings are NOT included in the DEBUG record

#### Scenario: DEBUG on vault insert

- **WHEN** `PiiVault::insert` adds a new mapping
- **THEN** a `tracing::debug!` call is emitted with fields: `conv_id`, `entity_type`, `original`, `synthetic`, `mapping_count` (total entries after insert)

#### Scenario: WARN on vault reload

- **WHEN** `PiiVault::reload` loads mappings from disk
- **THEN** a `tracing::warn!` call is emitted on success with fields: `vault_path`, `mapping_count`
- **AND** a `tracing::warn!` call is emitted on file error with fields: `vault_path`, `err = %e`
- **AND** no format-string interpolation is used in any of these calls

#### Scenario: TRACE on vault reload per-entry

- **WHEN** `RUST_LOG=trace` is set and `PiiVault::reload` restores entries from disk
- **THEN** a `tracing::trace!` call is emitted for each key-value pair restored, with fields: `entity_type`, `original_len`, `synthetic_len`

---

### Requirement: detect_spans Per-Tier Summary

The `detect_spans` orchestrator in `src/pii/mod.rs` SHALL emit a DEBUG summary for each tier that runs.

#### Scenario: DEBUG count per active tier

- **WHEN** `detect_spans` invokes any of Tier 1, Tier 2, or Tier 3
- **THEN** after each tier completes, a `tracing::debug!` call is emitted with fields: `tier` (1, 2, or 3), `count` (spans returned by that tier)
- **AND** a final `tracing::debug!` call is emitted after deduplication with `count` = total unique spans

#### Scenario: No log call when tier is disabled

- **WHEN** a tier is disabled in config (`pii.tiers.ner = false`)
- **THEN** no `tracing` call is emitted for that tier's result

