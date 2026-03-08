## ADDED Requirements

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
