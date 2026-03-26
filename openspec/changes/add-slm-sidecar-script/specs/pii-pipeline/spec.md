## ADDED Requirements

### Requirement: SLM Sidecar Script

The system SHALL provide a Python 3 executable script `packaging/privacyclaw-slm-sidecar`
that acts as a gateway between the Rust proxy and the bundled llama-server binary.
The script MUST listen on `127.0.0.1:16442` by default (overridable via `SIDECAR_PORT`
env var) and expose three HTTP endpoints consumed by the Rust `SlmSidecar` client.

#### Scenario: Sidecar starts and becomes healthy

- **WHEN** the sidecar is launched with `LLAMA_SERVER_PATH` and `MODEL_PATH` set
- **THEN** it spawns llama-server as a child subprocess on an internal port (default 8080)
- **AND** it polls `GET {LLAMA_ENDPOINT}/health` every 5 seconds until the subprocess is ready
- **AND** `GET /health` returns HTTP 200 `{"status": "ok"}` once llama-server is ready
- **AND** `GET /health` returns HTTP 503 `{"status": "starting"}` before readiness

#### Scenario: Sidecar starts without subprocess management

- **WHEN** `LLAMA_SERVER_PATH` is empty or not set
- **THEN** the sidecar starts without spawning any subprocess
- **AND** all three endpoints are available immediately (llama-server assumed external)

#### Scenario: llama-server subprocess crashes and is restarted

- **WHEN** the llama-server subprocess exits unexpectedly
- **THEN** the sidecar detects this via 3 consecutive failed health polls
- **AND** restarts the subprocess with exponential backoff (5 s, 10 s, 20 s, max 40 s)
- **AND** logs a WARN message with the exit code and restart schedule

### Requirement: POST /replace Endpoint

The sidecar SHALL implement `POST /replace` that detects PII in a text string using
the local SLM, resolves byte offsets with overlap-aware deduplication, generates
deterministic hash-seeded synthetic display values, and returns a structured response.

#### Scenario: Successful PII detection

- **WHEN** `POST /replace` is called with `{"text": "My name is Anne Nicole, phone 333-444-5555", "conversation_id": "abc"}`
- **AND** llama-server returns `["Anne Nicole", "333-444-5555"]`
- **THEN** the response is HTTP 200 with `replacements` containing two entries
- **AND** each entry has non-empty `display_value`, correct `pii_type`, and byte offsets matching the positions in `text`
- **AND** `modified_text` is always `""`
- **AND** `token_id` is always `""`

#### Scenario: Overlapping spans deduplicated

- **WHEN** llama-server returns both `["Anne Nicole", "Anne"]` and the text is `"Anne Nicole said hello"`
- **THEN** `resolve_replacements` returns only one replacement for `"Anne Nicole"` (longest match)
- **AND** the second `"Anne"` substring is skipped as it is covered by the longer match

#### Scenario: Text size limit enforced

- **WHEN** `POST /replace` is called with `text` longer than 32,768 characters
- **THEN** the sidecar returns HTTP 400 with `{"detail": "text too large"}`

#### Scenario: Fail-open on LLM error

- **WHEN** llama-server is not running or times out during `/replace`
- **THEN** the sidecar returns HTTP 200 with `{"modified_text": "", "replacements": []}`
- **AND** a WARNING is logged (raw text content is NOT included in the log message)

### Requirement: POST /v1/chat/completions Proxy Pass-Through

The sidecar SHALL proxy `POST /v1/chat/completions` to the internal llama-server
endpoint, supporting both streaming and non-streaming responses. This is required
for `SlmSidecar::disambiguate()` in the Rust proxy.

#### Scenario: Non-streaming completions proxied

- **WHEN** `POST /v1/chat/completions` is called without `"stream": true`
- **THEN** the sidecar forwards the request body unchanged to `{LLAMA_ENDPOINT}/v1/chat/completions`
- **AND** returns llama-server's response body and status code unchanged

#### Scenario: Streaming completions proxied

- **WHEN** `POST /v1/chat/completions` is called with `"stream": true`
- **THEN** the sidecar returns a `StreamingResponse` with `Content-Type: text/event-stream`
- **AND** chunks are forwarded from llama-server to the client as they arrive

#### Scenario: llama-server unavailable for completions

- **WHEN** llama-server is not reachable during a `/v1/chat/completions` request
- **THEN** the sidecar returns HTTP 503
- **AND** the Rust caller (`disambiguate()`) handles non-200 as fail-open (returns candidates unchanged)

### Requirement: Synthetic Display Value Generation

The sidecar SHALL generate deterministic hash-seeded synthetic display values for
each detected PII string. The same input string MUST always produce the same
synthetic value so that multiple detections of the same PII within a request are
consistent.

#### Scenario: Person name synthetic

- **WHEN** `generate_synthetic("Anne Nicole", "person_name")` is called
- **THEN** it returns one of the 50 hardcoded name pool entries
- **AND** calling it again with the same input returns the same name

#### Scenario: Email synthetic

- **WHEN** `generate_synthetic("user@corp.com", "email")` is called
- **THEN** it returns a string matching `redacted[0-9a-f]{4}@example.com`

#### Scenario: Fallback synthetic

- **WHEN** `generate_synthetic("unknown-pii-value", "other_pii")` is called
- **THEN** it returns `"[REDACTED]"`

### Requirement: Sidecar Dependency Guard

The sidecar script SHALL check for required Python packages at startup and exit
with code 1 with a clear `pip install` message if any package is missing.

#### Scenario: Missing dependency detected at startup

- **WHEN** the sidecar is run without `fastapi`, `uvicorn`, `httpx`, or `pydantic` installed
- **THEN** it prints `"Missing dependency: <package>"` and `"Install with: pip install fastapi uvicorn httpx pydantic"` to stdout
- **AND** exits with code 1 before binding any port
