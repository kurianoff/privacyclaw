## MODIFIED Requirements

### Requirement: Tiered PII Detection Pipeline

The system SHALL implement a three-tier PII detection pipeline that runs on the fully-buffered outbound request body. **Tier 3 (SLM) runs as a first pass on raw text; Tier 1 (regex) and Tier 2 (GLiNER NER) run after Tier 3 on the T3-modified text, skipping spans already replaced by T3.** Later tiers SHALL skip spans detected by earlier tiers. The pipeline SHALL be configurable per the activation matrix below.

Activation matrix:

| T1 | T2 | T3 | Behaviour |
|---|---|---|---|
| on | off | off | T1 regex → replace. |
| on | on | off | T1+T2 merged spans → replace. |
| off | off | on | T3 first pass → replace. No T1/T2 follow-up. |
| on | off | on | T3 first pass; then T1 on T3-modified text with exclusion zones. |
| on | on | on | T3 first pass; then T1+T2 on T3-modified text with exclusion zones; then T3 /disambiguate for low-confidence spans. |

T2 without T1 is INVALID. All other combinations are valid.

#### Scenario: Tier 1 only (default)

- **WHEN** `pii.tiers.regex = true` and `pii.tiers.ner = false` and `pii.tiers.slm = false`
- **AND** the request body contains `"email john@acme.com, SSN 123-45-6789"`
- **THEN** the email and SSN are detected and replaced within <2ms added latency
- **AND** the modified body is forwarded to the LLM

#### Scenario: Tier 1+2 enabled

- **WHEN** `pii.tiers.ner = true` and GLiNER model is installed
- **AND** the request body contains `"Ask Maria Johnson at 42 Oak Street"`
- **THEN** Tier 1 finds no structured PII
- **AND** Tier 2 (GLiNER) detects `[PERSON: Maria Johnson]` and `[LOCATION: 42 Oak Street]`
- **AND** both are replaced with synthetic XML tokens

#### Scenario: T3 standalone (first pass only, no T1/T2)

- **WHEN** `pii.tiers.slm = true` and `pii.tiers.regex = false` and `pii.tiers.ner = false`
- **AND** the request body contains `"Meet Anne Nicole at the Zurich office"`
- **THEN** `SlmSidecar::replace()` is called with the raw text
- **AND** T1 and T2 are NOT invoked
- **AND** detected entities are replaced with XML tokens `<pii id="TOKEN_ID">DISPLAY_VALUE</pii>`

#### Scenario: T3+T1 with exclusion zones

- **WHEN** `pii.tiers.slm = true` and `pii.tiers.regex = true` and `pii.tiers.ner = false`
- **AND** T3 detects and replaces "Anne Nicole" with an XML token in Stage 1
- **THEN** T1 runs on the T3-modified text
- **AND** the XML token span is excluded from T1 detection (no double-replacement)

#### Scenario: Tier 2 timeout fallback

- **WHEN** Tier 2 inference exceeds 500ms
- **THEN** a warning is logged (`tracing::warn`)
- **AND** only Tier 1 results are used for replacement
- **AND** the request is forwarded without further delay

#### Scenario: T3 /replace endpoint unavailable

- **WHEN** `SlmSidecar::replace()` returns `None` (timeout, non-200, or parse error)
- **THEN** Stage 1 is skipped — no T3 replacements are made
- **AND** T1/T2 (if enabled) run on the original unmodified text
- **AND** a `WARN` is logged

#### Scenario: PII mode off

- **WHEN** `pii.mode = "off"` (the default)
- **THEN** the pipeline is not invoked
- **AND** request bytes are forwarded byte-identical to upstream

---

### Requirement: Unified XML Token Format

All synthetic replacements, regardless of detecting tier, SHALL use the format `<pii id="TOKEN_ID">DISPLAY_VALUE</pii>`. TOKEN_ID is an 8-character base62 string computed from SHA-256(conversation_id + ":" + entity_index)[0..6 bytes]. DISPLAY_VALUE is a format-preserving synthetic value from `synth.rs` (unchanged generator).

The XML token is written into the request body text. `PiiDetection.synthetic` (used for dashboard display) carries only the bare `DISPLAY_VALUE`, not the XML token.

#### Scenario: XML token in forwarded body

- **WHEN** T1 detects `"john@acme.com"` with entity_index 0 in conversation `"conv-abc"`
- **THEN** the forwarded body contains `<pii id="TOKEN_ID">alice.brown@example.com</pii>` where TOKEN_ID is base62(SHA256("conv-abc:0")[0..6])
- **AND** `PiiDetection.synthetic` is `"alice.brown@example.com"` (bare display value)

#### Scenario: Same entity same token across turns

- **WHEN** "john@acme.com" is detected in Turn 1 and the vault mapping already exists when Turn 3 is processed
- **THEN** the same TOKEN_ID and DISPLAY_VALUE are emitted in Turn 3
- **AND** `SyntheticGenerator::get_or_create` returns the same display value idempotently

#### Scenario: Token ID uniqueness within conversation

- **WHEN** two different PII entities (e.g., an email and a name) are detected in the same conversation
- **THEN** they receive different TOKEN_IDs
- **AND** the TOKEN_ID for each is deterministic based on conversation_id and entity_index

---

### Requirement: System Instruction Injection

When PII mode is `"replace"`, the proxy SHALL inject a `SYSTEM_REMINDER` instruction into the outbound request body instructing the LLM to treat `<pii>` elements as atomic units and reproduce them verbatim. The injection gate SHALL be `pii.mode == Replace` regardless of which tiers are active.

#### Scenario: System instruction injected in Replace mode

- **WHEN** `pii.mode = "replace"` with any tier combination
- **THEN** the `SYSTEM_REMINDER` text is appended to the system prompt (Anthropic) or prepended as a system message (OpenAI)
- **AND** the instruction references the `<pii id="...">...</pii>` format

#### Scenario: No injection in detect-only mode

- **WHEN** `pii.mode = "detect-only"`
- **THEN** `SYSTEM_REMINDER` is NOT injected into the request body

#### Scenario: No injection when PII mode is off

- **WHEN** `pii.mode = "off"`
- **THEN** no modification to the request body

---

### Requirement: Streaming Inbound Reverse Replacement

After the LLM responds with a streaming SSE response containing XML tokens, the proxy SHALL replace XML tokens back to original PII values in real time before forwarding to the client. The `ReplacementBuffer` SHALL implement a five-level cascade matcher.

#### Scenario: XML token reversed via Level 1 (exact match)

- **WHEN** the SSE stream delivers the full XML token `<pii id="a3f9b2c1">Maria Blinke</pii>` that was emitted in the request
- **THEN** the buffer resolves it via `full_token_to_original` HashMap (O(1) lookup)
- **AND** the client receives the original PII value

#### Scenario: XML token reversed via Level 2 (ID-only match)

- **WHEN** the LLM slightly reformats the XML but preserves the `id` attribute (e.g., different whitespace)
- **THEN** the buffer extracts `id="TOKEN_ID"` and calls `vault.get_by_token_id()`
- **AND** the client receives the original PII value

#### Scenario: XML token reversed via Level 3 (display value match)

- **WHEN** the LLM strips the XML tags and emits only the bare display value `"Maria Blinke"`
- **THEN** the existing Aho-Corasick (Level 5) catches the bare synthetic
- **AND** the client receives the original PII value

#### Scenario: Level 4 stub passes through

- **WHEN** no Level 1–3 match is found for a `<pii>` token
- **THEN** a `WARN` is logged ("cascade Level 4 not yet implemented")
- **AND** the XML token is passed through unchanged to the client

#### Scenario: XML token split across SSE chunks

- **WHEN** `<pii id="a3f9` arrives in one SSE chunk and `b2c1">Maria Blinke</pii>` in the next
- **THEN** the buffer holds back the partial `<pii` sequence
- **AND** the complete token is resolved after the second chunk arrives
- **AND** the client receives the original value without the split being visible

#### Scenario: Bare synthetic reversed via Level 5 (Aho-Corasick)

- **WHEN** the LLM emits a bare display value `"Maria Blinke"` without XML tags
- **THEN** the Aho-Corasick automaton (built over display values) matches it
- **AND** the client receives the original PII value
- **AND** the XML-token holdback path does NOT interfere (the `['<','p']` prefix is not in `trigger_prefixes`)

---

## REMOVED Requirements

### Requirement: T3 Standalone Mode (slm_standalone flag)

**Reason**: The `slm_standalone: bool` field in `PiiPipeline` is superseded by the activation matrix. T3-only behaviour is now expressed as `{tiers.slm=true, tiers.regex=false, tiers.ner=false}`. The standalone flag concept is retired.

**Migration**: Remove all references to `slm_standalone` in `PiiPipeline`, `PiiPipeline::new()`, `PiiPipeline::tier1_only()`, and `c2u.rs`. Tests for the old `slm_standalone` flag must be deleted and replaced by activation-matrix routing tests.

---

## REMOVED Requirements

### Requirement: T3 detect_and_rewrite (§ delimiter format)

**Reason**: `detect_and_rewrite`, `extract_token_pairs`, and `SYSTEM_PROMPT_STANDALONE` in `tier3.rs` used a fragile `§value§` delimiter format. This is fully retired in favour of the XML token format and the new `/replace` endpoint.

**Migration**: Delete these functions and their tests. Replace with tests for `SlmSidecar::replace()`.
