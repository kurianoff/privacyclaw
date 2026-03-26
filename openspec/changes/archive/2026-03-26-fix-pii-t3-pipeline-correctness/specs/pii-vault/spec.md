## MODIFIED Requirements

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

## ADDED Requirements

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
