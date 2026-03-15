# storage Specification

## Purpose
TBD - created by archiving change add-privacyclaw-mvp. Update Purpose after archive.
## Requirements
### Requirement: Conversation Storage

The system SHALL persist intercepted LLM conversations to a SQLite database using the schema: `conversations(id, started_at, provider, model, client_hint)` and `messages(id, conversation_id, direction, timestamp, role, content, raw_http, tokens_in, tokens_out)`.

#### Scenario: New conversation created

- **WHEN** the first request of a new conversation is intercepted
- **THEN** a row is inserted into `conversations` with a UUID, timestamp, provider, model, and client_hint derived from User-Agent

#### Scenario: Message stored

- **WHEN** a request or response is parsed
- **THEN** a row is inserted into `messages` with direction (`request`/`response`), role, content, and compressed raw HTTP bytes

#### Scenario: Conversation grouping

- **WHEN** multiple requests arrive from the same client TCP connection within a short time window
- **THEN** they are grouped under the same conversation ID

### Requirement: Automatic Data Pruning

The system SHALL automatically delete conversations older than the configured `retention_days` and prune the database when it exceeds `max_size_mb`.

#### Scenario: Retention-based pruning

- **WHEN** a conversation's `started_at` timestamp is older than `retention_days`
- **THEN** the conversation and all its messages are deleted from the database

### Requirement: Conversation Query API

The storage module SHALL provide functions to list conversations (most recent first) and retrieve all messages for a given conversation ID.

#### Scenario: List conversations

- **WHEN** the dashboard requests the conversation list
- **THEN** conversations are returned ordered by `started_at` descending

#### Scenario: Get conversation messages

- **WHEN** the dashboard requests messages for a conversation ID
- **THEN** all messages for that conversation are returned ordered by `timestamp` ascending

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

### Requirement: Fingerprint-Based Conversation Continuity
The store SHALL identify an existing conversation by provider + fingerprint and return its ID, enabling multi-turn conversations to be grouped under one record.

#### Scenario: Existing fingerprint returns same conv_id
- **WHEN** a conversation was previously inserted with fingerprint A for provider "anthropic"
- **THEN** `find_conversation_by_fingerprint("anthropic", A)` returns that conversation's ID

#### Scenario: Same fingerprint different provider is separate
- **WHEN** fingerprint A exists for provider "anthropic"
- **THEN** `find_conversation_by_fingerprint("openai", A)` returns None

#### Scenario: Unknown fingerprint returns None
- **WHEN** no conversation with the given fingerprint exists
- **THEN** `find_conversation_by_fingerprint` returns None

### Requirement: Request Message Count Accuracy
`count_request_messages` SHALL return the count of messages with `direction == "request"` only, excluding response messages.

#### Scenario: Mixed directions counted correctly
- **WHEN** a conversation has 5 request messages and 3 response messages stored
- **THEN** `count_request_messages` returns 5

### Requirement: Concurrent Write Safety
Concurrent `batch_insert_messages` calls to the same conversation file SHALL produce a valid NDJSON file where every message line is a complete, parseable JSON object.

#### Scenario: High-concurrency appends produce no corruption
- **WHEN** 10 concurrent tasks each append 20 messages to the same conversation file
- **THEN** the resulting file has exactly 1 header line + 200 message lines
- **AND** every message line parses successfully as a `Message`

### Requirement: Message PII Processing Fields

The `Message` struct SHALL carry two new optional fields to support visual tracing of PII replacements:

- `pii_processed: Option<bool>` — `Some(true)` when the PII pipeline ran on this message, `Some(false)` when it ran and found nothing, `None` for legacy messages stored before this feature.
- `content_masked: Option<String>` — the text content of the message as it was actually forwarded to the LLM (with PII replaced by synthetic tokens). `None` when no replacement occurred or when the message is a legacy record.

Both fields SHALL use `#[serde(default, skip_serializing_if = "Option::is_none")]` so existing NDJSON files remain valid.

#### Scenario: Request with PII replaced

- **WHEN** the PII pipeline detects entities in a request and replaces them
- **THEN** the stored `Message` for that request has `pii_processed: Some(true)`
- **AND** `content_masked` contains the text content extracted from the PII-replaced body

#### Scenario: Request with pipeline run, no PII found

- **WHEN** the PII pipeline runs on a request but detects no entities
- **THEN** the stored `Message` has `pii_processed: Some(false)`
- **AND** `content_masked` is `None` (original and sent text are identical)

#### Scenario: Multi-turn request body

- **WHEN** a request body contains N messages (multi-turn keep-alive)
- **THEN** each stored `Message` record gets its own `content_masked` from the corresponding replaced message at the same index
- **AND** if the replaced body fails to parse, all N records get `content_masked: None` with `pii_processed: Some(true)`

#### Scenario: Legacy message backward compatibility

- **WHEN** a `Message` record is loaded from a file written before this change
- **THEN** `pii_processed` deserializes as `None`
- **AND** the dashboard shows an `(approx)` badge on column 2 of the compare view

---

### Requirement: StoredVaultRecord Confidence Field

`StoredVaultRecord` SHALL include a `confidence: Option<f32>` field so that detection confidence is preserved across proxy restarts and available in historical session views.

#### Scenario: Confidence persisted with vault

- **WHEN** the vault is saved after a response completes
- **THEN** each `StoredVaultRecord` includes the `confidence` value from the corresponding `PiiDetection`

#### Scenario: Legacy vault record without confidence

- **WHEN** a vault record is loaded from a file that predates this change
- **THEN** `confidence` deserializes as `None`
- **AND** the dashboard renders a dash (`—`) instead of a confidence bar for that entity

---

### Requirement: Per-Message Detection Log

The storage system SHALL persist per-message PII detection events as `"type":"detection"` lines in the conversation NDJSON file. Each line records one detected entity and the ID of the `Message` that triggered the detection.

Detection line format:
```json
{"type":"detection","message_id":"<uuid>","entity_type":"EMAIL","original_masked":"[EMAIL]","synthetic":"alice.brown@example.com","tier":1,"confidence":1.0}
```

`original_masked` SHALL be the type label (e.g. `[EMAIL]`, `[NAME]`) — NOT the plaintext original value.

The detection log is **append-only** and is never deduplicated. The same entity appearing in multiple turns produces one detection record per turn.

#### Scenario: Detection written for new entity

- **WHEN** the PII pipeline detects "alice@acme.com" for the first time in turn 1
- **THEN** a detection record is written with `message_id` = turn 1's message ID
- **AND** the vault maps `alice@acme.com → alice.brown@example.com`

#### Scenario: Detection written for recurring entity

- **WHEN** "alice@acme.com" appears again in turn 5
- **THEN** a second detection record is written with `message_id` = turn 5's message ID
- **AND** the vault is unchanged (idempotent — existing mapping reused)
- **AND** the turn 5 sidebar shows the detection correctly

#### Scenario: Load detections for a specific message

- **WHEN** `Store::load_detections(conv_id, Some(message_id))` is called
- **THEN** only records matching that `message_id` are returned
- **AND** the response includes `entity_type`, `original_masked`, `synthetic`, `tier`, `confidence`

#### Scenario: Load all detections for a conversation

- **WHEN** `Store::load_detections(conv_id, None)` is called
- **THEN** all detection records for the conversation are returned in file order

#### Scenario: No detection lines in legacy file

- **WHEN** `Store::load_detections` is called on a file with no `"type":"detection"` lines
- **THEN** an empty vector is returned without error

