## ADDED Requirements

### Requirement: Vault Persistence in NDJSON Files

The storage system SHALL persist `PiiVault` state as a JSON line with discriminator `"type":"vault"` appended to the conversation's NDJSON file. The vault line SHALL appear after the conversation header (line 1) and before any message lines.

Vault line format:
```json
{"type":"vault","rng_seed":12345678,"mappings":[{"original":"john@acme.com","synthetic":"alice.brown@example.com","pii_type":"Email"},...],"created_at":"2026-03-04T10:00:00Z"}
```

#### Scenario: Vault written on conversation completion

- **WHEN** `Store::save_vault(conv_id, vault)` is called after an SSE stream completes
- **THEN** the vault line is appended to the conversation file
- **AND** subsequent calls to `save_vault` for the same conversation overwrite the existing vault line (not append a duplicate)

#### Scenario: Vault read on conversation resume

- **WHEN** `Store::load_vault(conv_id)` is called
- **THEN** the system scans the conversation file for a line with `"type":"vault"`
- **AND** returns the deserialized `SavedVault` if found

#### Scenario: Phase 1 conversation files unaffected

- **WHEN** a conversation file contains no vault line (Phase 1 data)
- **THEN** `Store::load_vault()` returns `None` without error
- **AND** the file is not modified

#### Scenario: Vault line integrity

- **WHEN** the vault line JSON is malformed (e.g., truncated by a crash)
- **THEN** `Store::load_vault()` returns `None` and logs a warning
- **AND** the conversation messages are still accessible
