## ADDED Requirements

### Requirement: XML Token Index Structures

`PiiVault` SHALL maintain three HashMap index structures populated on every `add_mapping_with_token_id` call, enabling the ReplacementBuffer cascade matcher to resolve XML tokens back to original PII values at all levels.

- `full_token_to_original: HashMap<String, String>` — keyed on the exact emitted XML token string `<pii id="TOKEN_ID">DISPLAY_VALUE</pii>`, value is `original`. Supports cascade Level 1.
- `token_id_to_original: HashMap<String, String>` — keyed on TOKEN_ID (8 chars), value is `original`. Supports cascade Level 2.
- `display_value_to_original: HashMap<String, String>` — keyed on bare display value, value is `original`. Supports cascade Level 3.

All three SHALL be populated atomically in `add_mapping_with_token_id` and SHALL be restored from persisted vault records in `from_records()` when `token_id` is non-empty.

#### Scenario: All three maps populated on insert

- **WHEN** `add_mapping_with_token_id("john@acme.com", "alice.brown@example.com", "a3f9b2c1", PiiType::Email, 1, 1.0)` is called
- **THEN** `full_token_to_original[<pii id="a3f9b2c1">alice.brown@example.com</pii>]` == `"john@acme.com"`
- **AND** `token_id_to_original["a3f9b2c1"]` == `"john@acme.com"`
- **AND** `display_value_to_original["alice.brown@example.com"]` == `"john@acme.com"`

#### Scenario: Maps restored from persisted records

- **WHEN** a `VaultRecord` with `token_id = "a3f9b2c1"` and `synthetic = "alice.brown@example.com"` is loaded via `from_records()`
- **THEN** all three maps are populated with the correct entries
- **AND** `get_by_token_id("a3f9b2c1")` returns `Some("john@acme.com")`

#### Scenario: Legacy records without token_id are skipped

- **WHEN** a `VaultRecord` with `token_id = ""` (default) is loaded
- **THEN** only `display_value_to_original` is populated (for Level 3 / Level 5 fallback)
- **AND** `token_id_to_original` and `full_token_to_original` are NOT populated for that record

---

### Requirement: Token ID Generation

`PiiVault` SHALL expose a `generate_token_id(conversation_id: &str, entity_index: u64) -> String` helper function. The output is an 8-character base62 string derived from SHA-256(conversation_id + ":" + entity_index_decimal), taking the first 6 bytes of the hash and encoding them in base62 (`0-9A-Za-z` alphabet). No new crate dependencies are introduced — `sha2` is already present.

#### Scenario: Deterministic output for same inputs

- **WHEN** `generate_token_id("conv-abc", 0)` is called twice
- **THEN** both calls return the identical 8-character string

#### Scenario: Distinct output for different entity_index

- **WHEN** `generate_token_id("conv-abc", 0)` and `generate_token_id("conv-abc", 1)` are called
- **THEN** they return different 8-character strings

#### Scenario: Distinct output for different conversation_id

- **WHEN** `generate_token_id("conv-abc", 0)` and `generate_token_id("conv-xyz", 0)` are called
- **THEN** they return different 8-character strings

#### Scenario: Output is exactly 8 characters

- **WHEN** `generate_token_id` is called with any valid inputs
- **THEN** the returned string has length exactly 8
- **AND** all characters are in the set `0-9A-Za-z`

---

### Requirement: add_mapping_with_token_id Method

`PiiVault` SHALL expose `pub fn add_mapping_with_token_id(original, display_value, token_id, pii_type, tier, confidence)` which inserts a mapping with an externally-computed token_id. This complements the existing `add_mapping` (which generates no token_id) and is the primary insertion path for the T3-first pipeline.

#### Scenario: Full insert with token_id

- **WHEN** `add_mapping_with_token_id("Anne Nicole", "Maria Blinke", "a3f9b2c1", PersonName, 3, 1.0)` is called
- **THEN** `VaultRecord { original: "Anne Nicole", synthetic: "Maria Blinke", token_id: "a3f9b2c1", ... }` is stored
- **AND** all three index HashMaps are populated
- **AND** the Aho-Corasick `reverse_automaton` is rebuilt over display values (including "Maria Blinke")
- **AND** `mapping_count()` increases by 1

#### Scenario: Idempotent: same original maps to same synthetic

- **WHEN** `add_mapping_with_token_id` is called twice with the same `original`
- **THEN** only one entry is stored (idempotent, matching existing `add_mapping` behaviour)
- **AND** the second call is a no-op

---

### Requirement: Cascade Lookup Methods

`PiiVault` SHALL expose `get_by_token_id(token_id: &str) -> Option<&str>` and `get_by_display_value(display_value: &str) -> Option<&str>` for cascade Level 2 and Level 3 lookups respectively.

#### Scenario: get_by_token_id hit

- **WHEN** a mapping with token_id `"a3f9b2c1"` exists in the vault
- **AND** `get_by_token_id("a3f9b2c1")` is called
- **THEN** `Some("john@acme.com")` is returned

#### Scenario: get_by_token_id miss

- **WHEN** no mapping with token_id `"zzzzzzzz"` exists
- **AND** `get_by_token_id("zzzzzzzz")` is called
- **THEN** `None` is returned

#### Scenario: get_by_display_value hit

- **WHEN** a mapping with display_value `"alice.brown@example.com"` exists
- **AND** `get_by_display_value("alice.brown@example.com")` is called
- **THEN** `Some("john@acme.com")` is returned

---

## MODIFIED Requirements

### Requirement: Aho-Corasick Reverse Automaton

The vault SHALL maintain an Aho-Corasick multi-pattern search automaton built over all **display values** (bare synthetic strings). The automaton SHALL be rebuilt whenever a new mapping is added. It SHALL use `MatchKind::LeftmostLongest` to correctly handle overlapping synthetic tokens. The automaton is used exclusively for Level 5 cascade matching (bare synthetic in LLM response text). It is NOT built over XML token strings.

#### Scenario: Automaton rebuilt on mapping addition

- **WHEN** `PiiVault::add_mapping()` or `add_mapping_with_token_id()` is called
- **THEN** the `reverse_automaton` field is reconstructed from all current display values (bare synthetics)
- **AND** `max_synthetic_key_len` is updated to reflect the new maximum display-value length

#### Scenario: Correct longest-match semantics

- **WHEN** the vault contains both `"Alice"` and `"Alice Brown"` as display values
- **AND** the text `"Alice Brown"` is processed by the reverse automaton
- **THEN** the entire token `"Alice Brown"` is matched as a single entity
- **AND** it is replaced with the original corresponding to `"Alice Brown"` — not `"Alice"`

#### Scenario: XML token prefix not in trigger set

- **WHEN** a vault mapping is inserted via `add_mapping_with_token_id`
- **THEN** `synthetic_key_prefixes()` returns 2-byte prefixes of display values only
- **AND** the prefix `[b'<', b'p']` is NOT in the returned iterator (XML tags are not synthetic keys)
