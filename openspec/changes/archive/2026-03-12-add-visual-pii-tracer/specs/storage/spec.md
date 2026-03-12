## ADDED Requirements

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
