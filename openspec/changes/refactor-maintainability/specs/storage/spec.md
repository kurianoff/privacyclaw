## MODIFIED Requirements

### Requirement: Conversation Storage

The system SHALL persist intercepted LLM conversations to a SQLite database using the schema: `conversations(id, started_at, provider, model, client_hint)` and `messages(id, conversation_id, direction, timestamp, role, content, raw_http, tokens_in, tokens_out)`.

The `Store` implementation SHALL maintain an in-memory cache (`conv_cache: Arc<RwLock<HashMap<String, PathBuf>>>`) of conversation-ID-to-file-path mappings. `conv_file_path()` SHALL resolve a conversation's log file path in O(1) time from the cache rather than scanning the log directory on every call. The cache SHALL be populated at `Store::open()`, updated on `insert_conversation()`, and invalidated on `rotate_old()`.

#### Scenario: New conversation created

- **WHEN** the first request of a new conversation is intercepted
- **THEN** a row is inserted into `conversations` with a UUID, timestamp, provider, model, and client_hint derived from User-Agent
- **AND** the new conversation's file path is inserted into `conv_cache` at the same time

#### Scenario: Message stored with O(1) path lookup

- **WHEN** a request or response is parsed and stored
- **THEN** a row is inserted into `messages` with direction (`request`/`response`), role, content, and compressed raw HTTP bytes
- **AND** the log file path is resolved from `conv_cache` without scanning the log directory

#### Scenario: Conversation grouping

- **WHEN** multiple requests arrive from the same client TCP connection within a short time window
- **THEN** they are grouped under the same conversation ID

#### Scenario: Cache populated at open

- **WHEN** `Store::open()` is called on a directory containing existing conversation files
- **THEN** all existing conversation IDs are loaded into `conv_cache` before `open()` returns
- **AND** subsequent path lookups for those conversations do not touch the filesystem

#### Scenario: Cache invalidated on rotation

- **WHEN** `rotate_old()` removes conversation files older than the retention threshold
- **THEN** the corresponding entries are removed from `conv_cache`
- **AND** lookups for rotated conversation IDs fall back gracefully (returning `None` or an error)

---

## ADDED Requirements

### Requirement: Config Startup Validation

`Config::load()` SHALL call `Config::validate()` after successfully parsing the TOML file. `validate()` SHALL return an error if any of the following conditions hold:
- `proxy.listen` or `proxy.dashboard` cannot be parsed as a valid `SocketAddr`
- `network_proxy.listen` cannot be parsed as a valid `SocketAddr`
- `pii.ner.timeout_ms` or `pii.slm.timeout_ms` is zero
- `storage.logs_dir` is an empty string

Invalid configurations SHALL cause `privacyclaw start` and `privacyclaw network-start` to exit with a descriptive error before binding any ports.

#### Scenario: Valid config loads successfully

- **WHEN** a config file with valid addresses, positive timeouts, and a non-empty logs_dir is loaded
- **THEN** `Config::load()` returns `Ok(cfg)` and the proxy starts normally

#### Scenario: Invalid port rejected at startup

- **WHEN** `proxy.listen` is set to `"not-an-address"` in the config file
- **THEN** `Config::load()` returns `Err(…)` with a message identifying the invalid field
- **AND** `privacyclaw start` exits with a non-zero status before binding any port

#### Scenario: Zero timeout rejected at startup

- **WHEN** `pii.ner.timeout_ms` is set to `0` in the config file
- **THEN** `Config::load()` returns `Err(…)` with a message identifying the zero timeout
