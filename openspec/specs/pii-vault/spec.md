# pii-vault Specification

## Purpose
TBD - created by archiving change add-pii-protection. Update Purpose after archive.
## Requirements
### Requirement: PII Vault Lifecycle

The system SHALL maintain a per-conversation `PiiVault` containing bidirectional mappings between original PII values and their synthetic replacements. The vault SHALL be created on demand when the first PII entity is detected in a conversation and destroyed (from memory) after a configurable TTL (default 24 hours).

#### Scenario: Vault created on first PII detection

- **WHEN** the PII pipeline detects the first entity in a new conversation
- **THEN** a new `PiiVault` is created and registered in the `VaultRegistry` keyed by `conversation_id`
- **AND** the vault is seeded with a deterministic RNG derived from `sha256(conversation_id)[0..8]`

#### Scenario: Vault reused across turns

- **WHEN** subsequent API requests arrive for the same conversation (same `conversation_id`)
- **THEN** the same `PiiVault` instance is retrieved from the registry
- **AND** new PII found in later turns is added incrementally without changing existing mappings

#### Scenario: Vault TTL eviction

- **WHEN** a vault entry has not been accessed for longer than the configured TTL
- **THEN** it SHALL be evicted from the in-memory registry
- **AND** subsequent access for that `conversation_id` SHALL reload from storage (if persisted)

---

### Requirement: Bidirectional Mapping Consistency

Within a single conversation, the vault SHALL enforce 1-to-1 mapping: each original PII value maps to exactly one synthetic value, and each synthetic value maps back to exactly one original. The same original string SHALL always produce the same synthetic string within a session.

#### Scenario: Idempotent mapping

- **WHEN** the same original PII value (e.g., `"john@acme.com"`) is submitted for mapping twice
- **THEN** the vault returns the same synthetic value both times
- **AND** only one entry is stored in the bidirectional maps

#### Scenario: No collision on synthetic values

- **WHEN** two different original PII values of the same type are detected
- **THEN** they MUST receive different synthetic values
- **AND** the RNG advances deterministically so the second call produces a distinct output

---

### Requirement: Aho-Corasick Reverse Automaton

The vault SHALL maintain an Aho-Corasick multi-pattern search automaton built over all synthetic keys. The automaton SHALL be rebuilt whenever a new mapping is added. It SHALL use `MatchKind::LeftmostLongest` to correctly handle overlapping synthetic tokens (e.g., "Alice" is a prefix of "Alice Brown").

#### Scenario: Automaton rebuilt on mapping addition

- **WHEN** `PiiVault::add_mapping()` is called
- **THEN** the `reverse_automaton` field is reconstructed from all current synthetic keys
- **AND** `max_synthetic_key_len` is updated to reflect the new maximum

#### Scenario: Correct longest-match semantics

- **WHEN** the vault contains both `"Alice"` and `"Alice Brown"` as synthetic keys
- **AND** the text `"Alice Brown"` is processed by the reverse automaton
- **THEN** the entire token `"Alice Brown"` is matched as a single entity
- **AND** it is replaced with the original corresponding to `"Alice Brown"` — not `"Alice"`

---

### Requirement: Vault Persistence

The vault SHALL be persisted to the conversation's NDJSON storage file as a `"type":"vault"` line entry. On proxy restart, the vault SHALL be reloaded from storage when a conversation is resumed.

#### Scenario: Vault saved at stream completion

- **WHEN** an SSE response stream completes (message_stop or [DONE])
- **THEN** the current vault state for that conversation is serialized and appended/updated in the NDJSON file

#### Scenario: Vault reloaded on restart

- **WHEN** the proxy restarts and a new request arrives for a `conversation_id` that has a stored vault
- **THEN** the vault is reloaded with its previous mappings intact
- **AND** PII detected in the new turn gets the same synthetic value as in previous turns

#### Scenario: Vault not found in storage

- **WHEN** a request arrives for a `conversation_id` with no stored vault
- **THEN** a new empty vault is created
- **AND** no error is raised

---

### Requirement: VaultRegistry Thread Safety

The `VaultRegistry` SHALL be safe for concurrent access from multiple tokio tasks (one per active connection). All operations SHALL use `Arc<RwLock<PiiVault>>` per vault entry and a `Mutex<HashMap>` for the registry index.

#### Scenario: Concurrent access to the same vault

- **WHEN** two tasks concurrently process different requests in the same conversation
- **THEN** each acquires a write lock on the shared `VaultHandle` in turn
- **AND** neither mapping is lost

#### Scenario: Concurrent access to different vaults

- **WHEN** two tasks concurrently process requests for different conversations
- **THEN** they acquire independent `VaultHandle` locks
- **AND** neither task is blocked by the other

### Requirement: Synthetic Key Prefix Iterator

`PiiVault` SHALL expose a `synthetic_key_prefixes()` method that returns an iterator over the first 2 bytes of each synthetic key. Keys shorter than 2 bytes SHALL be skipped. This method is used by `ReplacementBuffer` to build a 2-byte prefix trigger set, replacing the previous single-character trigger set.

#### Scenario: Prefix iterator returns correct 2-byte slices

- **GIVEN** a vault containing synthetic keys `"fd1a2b:3c4d::1"`, `"alice.smith@example.com"`, `"10.23.45.67"`
- **WHEN** `synthetic_key_prefixes()` is called
- **THEN** the iterator yields `[b'f', b'd']`, `[b'a', b'l']`, `[b'1', b'0']` (order unspecified)

#### Scenario: Keys shorter than 2 bytes are skipped

- **GIVEN** a vault that somehow contains a 1-byte synthetic key `"x"`
- **WHEN** `synthetic_key_prefixes()` is called
- **THEN** that entry is excluded from the iterator result

### Requirement: IPv6 Synthetic Key Length

The synthetic IPv6 generator (`gen_ipv6()`) SHALL produce values using exactly 2 random 16-bit hex groups in the format `fd{g1}:{g2}::1`, resulting in a key length of at most 16 characters. This replaces the previous 7-group format which produced ~39-character keys that inflated `max_synthetic_key_len` and caused `ReplacementBuffer` to hold back streaming response text unnecessarily.

#### Scenario: IPv6 synthetic is short enough not to stall the buffer

- **GIVEN** a vault with only IPv6 mappings
- **WHEN** `max_synthetic_key_len` is queried
- **THEN** the value is ≤ 16

#### Scenario: IPv6 synthetic is still unique-local format

- **GIVEN** `gen_ipv6()` is called
- **THEN** the result starts with `fd` (RFC 4193 unique local prefix)
- **AND** the result ends with `::1`

### Requirement: Vault Confidence Storage

`PiiVault` SHALL store a `confidence: f32` value alongside each mapping. The value SHALL be sourced from the `PiiDetection.confidence` produced by the detection pipeline and persisted in `StoredVaultRecord.confidence`.

`PiiVault::add_mapping` and `insert_mapping_raw` SHALL accept a `confidence: f32` parameter. The vault SHALL maintain a `confidences: Vec<f32>` parallel vec alongside the existing `tiers`, `pii_types`, `original_values`, and `synthetic_keys` vecs.

#### Scenario: Confidence stored with mapping

- **WHEN** `add_mapping(original, synthetic, pii_type, tier, confidence)` is called
- **THEN** `confidences[i]` equals the passed confidence value for the new mapping at index `i`
- **AND** all parallel vecs remain the same length

#### Scenario: Confidence available in quints iterator

- **WHEN** the vault's `quints()` iterator is called
- **THEN** each element is `(&original, &synthetic, &pii_type_label, tier, confidence)`
- **AND** the confidence value matches what was passed to `add_mapping`

#### Scenario: Legacy record loaded without confidence

- **WHEN** a `StoredVaultRecord` with `confidence: None` is loaded via `from_records`
- **THEN** `confidences[i]` is set to `0.0` (sentinel for "confidence unknown")
- **AND** the mapping is otherwise functional

---

### Requirement: Vault Persistence Includes Confidence

`Store::save_vault` SHALL persist confidence, `token_id`, and `display_value` values in
each `StoredVaultRecord`. On reload, all three values SHALL be restored from storage and
used to repopulate the `full_token_to_original`, `token_id_to_original`, and
`display_value_to_original` index maps so that cascade L1/L2/L3 lookups function after
proxy restart.

`StoredVaultRecord` SHALL add two optional fields:
- `token_id: Option<String>` — persisted with `#[serde(default, skip_serializing_if = "Option::is_none")]` for backward compatibility
- `display_value: Option<String>` — same serde attributes

Existing NDJSON vault entries without these fields SHALL deserialise with `None` and load
correctly (cascade indexes empty for those legacy records only; Aho-Corasick Level 5
remains available as fallback).

#### Scenario: Round-trip confidence preservation

- **WHEN** a vault with confidence values is saved and then reloaded
- **THEN** each mapping's confidence value equals the original value
- **AND** the `quints()` iterator reflects the reloaded values

#### Scenario: Round-trip token_id and display_value preservation

- **WHEN** a vault entry with a non-empty `token_id` and `display_value` is saved and reloaded
- **THEN** `get_by_token_id(token_id)` returns the original PII value
- **AND** `get_by_display_value(display_value)` returns the original PII value
- **AND** the full XML token `<pii id="TOKEN_ID">DISPLAY_VALUE</pii>` is present in
  `full_token_to_original`

#### Scenario: Legacy vault entries load without cascade indexes

- **WHEN** a `StoredVaultRecord` deserialized from legacy NDJSON has no `token_id` or
  `display_value` fields
- **THEN** `token_id` and `display_value` default to empty/None
- **AND** the mapping is still loadable via Aho-Corasick (Level 5) on the synthetic value
- **AND** no error is raised during load

### Requirement: T3 Original PII Captured Before Replace

When processing T3 `/replace` responses, the pipeline SHALL capture the real original PII
text from `&text[r.start..r.end]` before calling `replace_range` on the working text.
The vault SHALL store the captured original (not a placeholder like `"T3_0"`). The
`PiiDetection.original` field SHALL also contain the real original text.

#### Scenario: T3 vault entry restores real original

- **WHEN** the SLM sidecar detects PII at offsets `[start, end)` in the input text
- **AND** the vault stores the mapping
- **THEN** the inbound restoration path returns the original text from `text[start..end]`
- **AND** `PiiDetection.original` reflects the same value, not a placeholder

### Requirement: Per-Conversation Token Scoping

The pipeline SHALL use the real `conversation_id` (from `vault.conversation_id`) when
calling `generate_token_id`. The literal string `"conv"` SHALL NOT be used as the
conversation identifier at any call site.

#### Scenario: Token IDs are scoped per conversation

- **WHEN** two conversations process the same PII entity at the same entity index
- **THEN** their generated `token_id` values differ
- **AND** vault lookups for one conversation do not interfere with the other

### Requirement: Cascade Index Maps Always Populated

`add_mapping_with_token_id` SHALL populate `full_token_to_original`,
`token_id_to_original`, and `display_value_to_original` on every call, even when the
original is already present in `original_to_synthetic` (idempotent duplicate). Only the
raw storage insert and `rebuild_automaton` SHALL be guarded by the duplicate check.

#### Scenario: Index maps populated after get_or_create

- **WHEN** `SyntheticGenerator::get_or_create` inserts a plain mapping via `add_mapping`
- **AND** `add_mapping_with_token_id` is subsequently called with the same original
- **THEN** `get_by_token_id(token_id)` returns the original PII value
- **AND** `get_by_display_value(display_value)` returns the original PII value

