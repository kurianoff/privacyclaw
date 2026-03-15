## ADDED Requirements

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
