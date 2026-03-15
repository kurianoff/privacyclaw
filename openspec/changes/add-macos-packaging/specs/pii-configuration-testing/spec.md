## ADDED Requirements

### Requirement: PII Mode Toggle Tests

The test suite SHALL verify that the `pii.mode` setting (`off` / `detect-only` / `replace`) produces distinct, correct outcomes across the proxy, dashboard, and logs. Each mode SHALL have dedicated unit and integration tests.

#### Scenario: Mode `off` — request body passes through unmodified

- **WHEN** `pii.mode = "off"` and a request body contains an email address
- **THEN** `PiiPipeline::process_request_body` is not called
- **AND** the outbound request bytes are identical to the original
- **AND** no `pii: detected span` log lines are emitted
- **AND** the dashboard PII panel shows "PII Protection: Off"

#### Scenario: Mode `detect-only` — spans logged but body unchanged

- **WHEN** `pii.mode = "detect-only"` and a request body contains a phone number and SSN
- **THEN** `pii: detected span` log lines are emitted for each span (entity_type, byte range, confidence, tier)
- **AND** the outbound request body is byte-for-byte identical to the original
- **AND** the dashboard PII panel shows detected entity types for the conversation without masking them

#### Scenario: Mode `replace` — body is modified, vault is populated

- **WHEN** `pii.mode = "replace"` and a request body contains an API key
- **THEN** the outbound body has the API key replaced with a synthetic value
- **AND** the vault contains a mapping from the original to the synthetic
- **AND** `pii: detected span` log lines are emitted
- **AND** the dashboard PII panel shows masked entities (e.g. `[API_KEY]`)

#### Scenario: Hot-reload from `off` to `replace` takes effect on next request

- **WHEN** the proxy is running with `pii.mode = "off"`
- **AND** a `PATCH /api/config` sets `pii.mode = "replace"` and `pii.tiers.regex = true`
- **THEN** the next intercepted request has PII replaced
- **AND** no restart is required

### Requirement: Tier 1 (Regex) Tests

The test suite SHALL provide unit tests covering every Tier 1 entity type and edge cases of the regex detector.

#### Scenario: Email detection

- **WHEN** a message contains `contact@example.com`
- **THEN** `Tier1Detector::detect` returns a span of type `Email` with confidence 1.0
- **AND** the replacement in `replace` mode contains no `@` character

#### Scenario: Phone number detection — E.164 and local formats

- **WHEN** a message contains `+1 (555) 867-5309` or `555-867-5309`
- **THEN** a span of type `Phone` is detected
- **AND** the replacement is a synthetic phone number of matching format

#### Scenario: SSN detection

- **WHEN** a message contains `123-45-6789`
- **THEN** a span of type `Ssn` is detected with confidence 1.0

#### Scenario: API key / Bearer token detection

- **WHEN** a message contains `sk-proj-abc123` or `Bearer eyJhbGciOiJSUzI1N`
- **THEN** a span of type `ApiKey` or `BearerToken` is detected
- **AND** the replacement does not include any part of the original key

#### Scenario: No false positives on benign text

- **WHEN** a message contains `the version is 1.2.3` or `call us at noon`
- **THEN** `Tier1Detector::detect` returns an empty span list

#### Scenario: Multiple entity types in one message

- **WHEN** a message contains both an email and an SSN
- **THEN** two non-overlapping spans are returned, one per entity type
- **AND** both are replaced independently in `replace` mode

#### Scenario: Tier 1 disabled — detector not invoked

- **WHEN** `pii.tiers.regex = false`
- **THEN** `Tier1Detector::detect` is not called during request processing
- **AND** the pipeline returns `None` immediately

### Requirement: Tier 2 (NER/GLiNER) Tests

The test suite SHALL verify Tier 2 behaviour both when the NER model is available and when it is absent, and SHALL use a stub/mock that avoids loading a real ONNX model in CI.

#### Scenario: Tier 2 disabled — NER not invoked

- **WHEN** `pii.tiers.ner = false`
- **THEN** `PiiPipeline.tier2` is `None`
- **AND** `detect_spans` returns only Tier 1 results

#### Scenario: Tier 2 without Tier 1 rejected at config layer

- **WHEN** a `PATCH /api/config` enables Tier 2 but Tier 1 is `false`
- **THEN** a 422 response is returned with `"error": "Tier 2 requires Tier 1"`
- **AND** the config is not persisted

#### Scenario: Tier 2 model absent — graceful fallback to Tier 1

- **WHEN** `pii.tiers.ner = true` but the model file at `pii.ner.model_path` does not exist
- **THEN** startup logs `WARN pii.tiers.ner = true but model not found; disabling Tier 2`
- **AND** the pipeline continues with Tier 1 only — no crash or error propagation

#### Scenario: Tier 2 detects person name missed by Tier 1

- **WHEN** using a `Tier2Detector` stub that returns a `PersonName` span for "Alice"
- **AND** the message contains "Please send the contract to Alice"
- **THEN** the merged span list includes the `PersonName` span
- **AND** in `replace` mode "Alice" is replaced with a synthetic name

#### Scenario: Tier 1 and Tier 2 spans merged without duplicates

- **WHEN** Tier 1 detects an email at bytes [10, 30] and Tier 2 detects an overlapping span at [12, 30]
- **THEN** `merge_spans` returns exactly one span covering the higher-confidence detection
- **AND** no double-replacement occurs in the output body

### Requirement: Tier 3 (SLM/llama-server) Tests

The test suite SHALL verify Tier 3 sidecar lifecycle, model selection, and disambiguation logic using a mock HTTP server (via `wiremock` or a hand-rolled `TcpListener` stub). No real llama-server binary is required for automated tests.

#### Scenario: Tier 3 disabled — sidecar not started

- **WHEN** `pii.tiers.slm = false`
- **THEN** `PiiPipeline.slm` is `None`
- **AND** no llama-server process is spawned
- **AND** all Tier 2 spans are treated as confirmed

#### Scenario: Tier 3 without Tier 1+2 rejected at config layer

- **WHEN** a `PATCH /api/config` enables Tier 3 but Tier 2 is `false`
- **THEN** a 422 response is returned with `"error": "Tier 3 requires Tier 1 and Tier 2"`

#### Scenario: Model must be downloaded before Tier 3 can activate

- **WHEN** `POST /api/models/llama-3.2-1b/activate` is called
- **AND** the model GGUF file is not present on disk
- **THEN** a 409 response is returned with `"error": "model not downloaded"`
- **AND** the Tier 3 sidecar is not started

#### Scenario: Model selection — sidecar restarts with new model

- **WHEN** model A is active (sidecar running) and `POST /api/models/:b/activate` is called
- **AND** model B is already downloaded
- **THEN** the existing sidecar process is stopped (verified via mock `SidecarProcess`)
- **AND** a new sidecar is started pointing at model B's GGUF path on port 16442
- **AND** `pii.slm.model_id` in the config is updated to model B's id

#### Scenario: Tier 3 disambiguates low-confidence Tier 2 spans via mock SLM

- **WHEN** a mock llama-server returns `[0]` (confirming index 0 of 2 candidates)
- **THEN** only the first candidate span is included in the final span list
- **AND** the second candidate is discarded

#### Scenario: Tier 3 fail-open on SLM timeout

- **WHEN** the llama-server does not respond within `pii.slm.timeout_ms`
- **THEN** all low-confidence candidate spans are treated as confirmed (fail-open)
- **AND** a WARN log is emitted: `Tier3: timeout contacting llama-server`
- **AND** request processing completes — no error propagated to the client

#### Scenario: Tier 3 fail-open on SLM HTTP error

- **WHEN** the mock llama-server returns HTTP 500
- **THEN** the original candidate spans are used unchanged (fail-open)
- **AND** a WARN log is emitted

### Requirement: Proxy Start/Stop Toggle Tests

The test suite SHALL verify that starting and stopping the proxy — via CLI (`privacyclaw start` / `privacyclaw stop`) and via the dashboard UI / API — correctly affects the proxy's listening state, dashboard indicators, and log output.

#### Scenario: `privacyclaw stop` terminates the proxy cleanly

- **WHEN** the proxy is running and `privacyclaw stop` is executed
- **THEN** a SIGTERM is sent to the proxy process (located via PID file)
- **AND** the process exits within 5 seconds
- **AND** the PID file is removed
- **AND** "Proxy stopped" is printed to stdout

#### Scenario: `privacyclaw stop` when proxy is not running

- **WHEN** `privacyclaw stop` is run and no PID file exists
- **THEN** "Proxy is not running" is printed and the command exits with code 0

#### Scenario: Dashboard stop button stops the proxy

- **WHEN** `POST /api/proxy/stop` is called from the dashboard
- **THEN** the proxy stops accepting new connections
- **AND** a `proxy_status` WebSocket event is broadcast with `{ "running": false }`
- **AND** the dashboard header status indicator turns red/inactive

#### Scenario: Dashboard start button restarts the proxy

- **WHEN** the proxy is stopped and `POST /api/proxy/start` is called
- **THEN** the HTTP proxy listener resumes on port 16440
- **AND** a `proxy_status` WebSocket event is broadcast with `{ "running": true }`

#### Scenario: PII mode change while proxy is running does not drop connections

- **WHEN** `PATCH /api/config` changes `pii.mode` from `off` to `replace`
- **AND** there are active client connections at the time
- **THEN** existing connections are drained at their current PII mode
- **AND** new connections use the updated mode
- **AND** no connection errors are returned to clients

#### Scenario: Proxy stop clears dashboard active sessions

- **WHEN** the proxy stops
- **THEN** all WebSocket dashboard clients receive a `proxy_status` event
- **AND** the conversation list in the dashboard is frozen (no new entries added)
