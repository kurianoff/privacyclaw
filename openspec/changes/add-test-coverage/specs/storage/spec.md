## ADDED Requirements

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
