# storage Specification

## Purpose
TBD - created by archiving change add-kladovka-mvp. Update Purpose after archive.
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

