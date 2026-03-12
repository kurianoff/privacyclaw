## MODIFIED Requirements

### Requirement: Three-Tier Structured Log Levels

The system SHALL instrument every source module with `tracing` calls at five distinct levels: TRACE, DEBUG, INFO, WARN, and ERROR, where each level carries a well-defined scope of information.

#### Scenario: TRACE level exposes every decision branch in the PII pipeline

- **WHEN** the subscriber's max level is TRACE (`RUST_LOG=claudovka=trace`)
- **THEN** every conditional branch, pattern match attempt, character-level buffer decision, and vault key lookup in the PII pipeline emits a `tracing::trace!` call with structured key=value fields identifying the branch outcome
- **AND** no log call allocates on the heap when the subscriber's configured level would filter the record out (the `tracing` crate's disabled-check guarantee)

#### Scenario: DEBUG level logs every code branch and raw data

- **WHEN** the subscriber's max level is DEBUG
- **THEN** every loop iteration, conditional branch, and I/O operation in every module emits a `tracing::debug!` call with structured key=value fields
- **AND** raw transmitted bytes are included as a `chunk_hex` field (lowercase hex, truncated to 256 bytes) with a `chunk_total_bytes` field for the full size
- **AND** HTTP `Authorization` and `x-api-key` header values are replaced with `"[REDACTED]"` in all log output

#### Scenario: INFO level logs every atomic operation

- **WHEN** the subscriber's max level is INFO or DEBUG
- **THEN** every meaningful unit of work emits a `tracing::info!` call: forwarded chunk (with byte count), received chunk, full request body received, response finalized, SSE event processed, storage record inserted/found, certificate cache hit/miss, HTTP request/response, DNS resolution result, and first-time PII entity generated
- **AND** each INFO record includes structured fields sufficient to identify the operation, the subject (host, conv_id, path), and any quantitative result (bytes, count, latency)

#### Scenario: WARN level logs every lifecycle event

- **WHEN** any of the following occur: proxy listener bound, proxy started in a particular mode, MITM session started/ended, passthrough established, WebSocket client connected/disconnected, CA generated or installed, graceful shutdown initiated, log rotation completed
- **THEN** a `tracing::warn!` call is emitted with the relevant context (addr, host, peer_addr, file count, etc.)
- **AND** events previously logged at INFO that match lifecycle semantics (listener bound, CA generated, CA installed) are moved to WARN

---

## ADDED Requirements

### Requirement: JSON Log Format

The system SHALL support a `json` log format that emits each log record as a single-line JSON object to the configured output sink(s). The `text` format (human-readable key=value) SHALL remain available.

The JSON shape SHALL be:
```json
{"ts":"2026-03-12T10:00:00.123456Z","level":"DEBUG","target":"claudovka::pii::tier1","msg":"pattern matched","entity_type":"Email","span_start":5,"span_end":22}
```

Fields:
- `ts` — RFC 3339 UTC timestamp with microsecond precision
- `level` — uppercase level name: `TRACE`, `DEBUG`, `INFO`, `WARN`, `ERROR`
- `target` — Rust module path (e.g. `claudovka::pii::synth`)
- `msg` — the static message string from the macro call
- Additional structured fields appended as sibling JSON keys

#### Scenario: JSON format selected via config

- **WHEN** `[logging].format = "json"` is set in `config.toml` (the default)
- **THEN** every log record emitted to stderr is a single-line JSON object matching the schema above
- **AND** no bare text lines are mixed into the stream

#### Scenario: Text format preserves existing behavior

- **WHEN** `[logging].format = "text"` is set
- **THEN** log output uses the existing human-readable `tracing_subscriber::fmt` default format
- **AND** structured fields appear as `key=value` pairs on the same line

#### Scenario: Invalid format value

- **WHEN** `[logging].format = "xml"` or any unrecognized value is set
- **THEN** the proxy logs a WARN and falls back to `"json"` format
- **AND** startup is not aborted

---

### Requirement: Optional File Log Output

The system SHALL support writing log records to a rolling file in addition to (not instead of) stderr. File output is disabled by default.

#### Scenario: File output enabled

- **WHEN** `[logging].file = "/var/log/claudovka/app.log"` is set
- **THEN** log records are written to that path in the configured format, in addition to stderr
- **AND** the file is created if it does not exist (parent directory must exist)

#### Scenario: Daily rotation

- **WHEN** `[logging].rotation = "daily"` (the default when `file` is set)
- **THEN** a new log file is created each UTC day, suffixed with the date (e.g. `app.log.2026-03-12`)
- **AND** the `tracing_appender::rolling::daily` builder is used
- **AND** a `WorkerGuard` is held in `main()` until process exit to flush the non-blocking writer

#### Scenario: File output disabled by default

- **WHEN** `[logging].file` is absent from the config
- **THEN** no log file is created and no file I/O occurs in the logging path

#### Scenario: CLI flag overrides config file path

- **WHEN** the user passes `--log-file /tmp/debug.log` on the command line
- **THEN** that path overrides any `[logging].file` value from the config
- **AND** rotation is applied using the config's `rotation` setting

---

### Requirement: Subscriber Initialized Before Config Load

The system SHALL initialize the `tracing` subscriber as the first action in `main()`, before any call to `Config::load` or any other subsystem initialization.

#### Scenario: Config load errors are visible in logs

- **WHEN** `Config::load` encounters a malformed TOML file and returns an error
- **THEN** that error is emitted via `tracing::error!` and appears in the log output
- **AND** no log records from the config-load path are silently dropped

#### Scenario: Subscriber uses a two-phase init

- **WHEN** `main()` starts with no config yet available
- **THEN** a preliminary subscriber is registered using `RUST_LOG` env var or a hardcoded `INFO` default
- **WHEN** `Config::load` succeeds and returns `cfg.logging.level`
- **THEN** the `EnvFilter` reload handle is used to update the filter to the configured level without restarting the subscriber

---

### Requirement: Structured Field Naming Conventions

All `tracing` call sites in the codebase SHALL use a consistent set of field names. No format-string interpolation (`"text {}", var`) is permitted — every variable MUST appear as a named structured field.

Field name registry:

| Field | Type | Description |
|---|---|---|
| `conv_id` | `%String` | Conversation identifier |
| `provider` | `%str` | LLM provider name (e.g. `"anthropic"`) |
| `host` | `%str` | Target hostname |
| `entity_type` | `%str` | PII entity type name (e.g. `"Email"`, `"Ssn"`) |
| `tier` | `u8` | Detection tier (1, 2, or 3) |
| `span_start` | `usize` | Byte offset of PII span start |
| `span_end` | `usize` | Byte offset of PII span end |
| `confidence` | `f32` | Detection confidence (0.0–1.0) |
| `original` | `%str` | Original PII text (INFO on first generation only) |
| `synthetic` | `%str` | Synthetic replacement text (INFO on first generation only) |
| `original_len` | `usize` | Byte length of original PII text |
| `text_len` | `usize` | Length of text being processed |
| `chunk_len` | `usize` | Byte count of a network chunk |
| `flushed_len` | `usize` | Bytes flushed from holdback buffer |
| `holdback_len` | `usize` | Current bytes held in buffer pending resolution |
| `mapping_count` | `usize` | Number of entries in the PII vault |
| `body_len` | `usize` | HTTP request/response body size |
| `count` | `usize` | Generic count (spans detected, events processed, etc.) |
| `err` | `%e` | Display-format error value |
| `detail` | `?e` | Debug-format error value (ERROR level only) |
| `payload` | `%str` | Truncated raw text for ERROR diagnostics (max 512 chars) |

#### Scenario: No format-string interpolation in any tracing call

- **WHEN** `cargo clippy -- -D warnings` is run on the project
- **THEN** no clippy lint fires for format arguments in tracing macro calls
- **AND** `rg 'tracing::(debug|info|warn|error|trace)!\("[^"]*\{' src/'` returns zero matches

#### Scenario: Field names match the registry at call sites

- **WHEN** a new call site is added to `src/pii/` or `src/proxy/`
- **THEN** field names are drawn from the registry above (or a new field is added to the registry via spec update)
- **AND** no ad-hoc unnamed fields (e.g. `tracing::debug!(some_var)` without a name) are used
