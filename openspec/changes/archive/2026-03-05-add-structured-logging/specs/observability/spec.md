## ADDED Requirements

### Requirement: Three-Tier Structured Log Levels

The system SHALL instrument every source module with `tracing` calls at three distinct levels: DEBUG, INFO, and WARN, where each level carries a well-defined scope of information.

#### Scenario: DEBUG level logs every code branch and raw data

- **WHEN** the subscriber's max level is DEBUG
- **THEN** every loop iteration, conditional branch, and I/O operation in every module emits a `tracing::debug!` call with structured key=value fields
- **AND** raw transmitted bytes are included as a `chunk_hex` field (lowercase hex, truncated to 256 bytes) with a `chunk_total_bytes` field for the full size
- **AND** HTTP `Authorization` and `x-api-key` header values are replaced with `"[REDACTED]"` in all log output

#### Scenario: INFO level logs every atomic operation

- **WHEN** the subscriber's max level is INFO or DEBUG
- **THEN** every meaningful unit of work emits a `tracing::info!` call: forwarded chunk (with byte count), received chunk, full request body received, response finalized, SSE event processed, storage record inserted/found, certificate cache hit/miss, HTTP request/response, DNS resolution result
- **AND** each INFO record includes structured fields sufficient to identify the operation, the subject (host, conv_id, path), and any quantitative result (bytes, count, latency)

#### Scenario: WARN level logs every lifecycle event

- **WHEN** any of the following occur: proxy listener bound, proxy started in a particular mode, MITM session started/ended, passthrough established, WebSocket client connected/disconnected, CA generated or installed, graceful shutdown initiated, log rotation completed
- **THEN** a `tracing::warn!` call is emitted with the relevant context (addr, host, peer_addr, file count, etc.)
- **AND** events previously logged at INFO that match lifecycle semantics (listener bound, CA generated, CA installed) are moved to WARN

### Requirement: Async-Safe Logging

The system SHALL NOT perform blocking I/O inside tracing macro calls or as a direct side-effect of tracing on the tokio runtime thread.

#### Scenario: Logging inside async task does not block executor

- **WHEN** a `tracing::debug!` macro is called inside a tokio-spawned async task
- **THEN** the call completes without blocking the executor thread
- **AND** the tracing subscriber writes asynchronously or to a non-blocking sink (stdout via `tracing_subscriber::fmt` is acceptable)

### Requirement: Sensitive Data Redaction at DEBUG

The system SHALL redact known sensitive HTTP header values from all log output, including DEBUG-level raw header dumps.

#### Scenario: Authorization header is redacted

- **WHEN** DEBUG logging is enabled and an HTTP request containing `Authorization: Bearer sk-...` passes through the proxy
- **THEN** the log output contains `Authorization: [REDACTED]` and does not contain the actual token value

#### Scenario: x-api-key header is redacted

- **WHEN** DEBUG logging is enabled and a request containing `x-api-key: sk-ant-...` is intercepted
- **THEN** the log output contains `x-api-key: [REDACTED]` and does not contain the actual key value

### Requirement: Raw Byte Logging with Truncation

The system SHALL log raw transmitted bytes at DEBUG level with a maximum field size to prevent runaway log volume.

#### Scenario: Large chunk is truncated in DEBUG output

- **WHEN** a 65 KB chunk is read from the upstream connection
- **THEN** the `chunk_hex` field in the DEBUG log contains at most 256 bytes (512 hex characters) of the chunk
- **AND** the `chunk_total_bytes` field contains the full untruncated byte count (e.g. 65536)

#### Scenario: Small chunk is logged in full

- **WHEN** a 100-byte chunk is read
- **THEN** the `chunk_hex` field contains the full 100-byte hex encoding (200 characters)
- **AND** no truncation suffix is appended
