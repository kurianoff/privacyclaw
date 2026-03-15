## ADDED Requirements

### Requirement: Configuration REST API

The dashboard server SHALL expose a configuration REST API to read and patch the live configuration. `GET /api/config` SHALL return the full sanitised current config as JSON. `PATCH /api/config` SHALL accept a partial config JSON, validate it (including PII tier dependency rules), persist the change to `config.toml`, apply it to the running process without restart where possible, and broadcast a `config_changed` WebSocket event. Changes to listening addresses (proxy port, dashboard port) SHALL be accepted and persisted but SHALL return a `restart_required: true` flag in the response.

#### Scenario: Read config

- **WHEN** a GET request is made to `/api/config`
- **THEN** a JSON object is returned with the current proxy, PII, and logging settings
- **AND** sensitive values (API keys, tokens) are not included in the response

#### Scenario: Patch PII tier setting

- **WHEN** a PATCH request is made to `/api/config` with `{ "pii": { "tiers": { "ner": true } } }`
- **AND** `pii.tiers.regex` is already `true`
- **THEN** the NER tier is enabled immediately and a `config_changed` WebSocket event is broadcast

#### Scenario: Tier dependency enforcement

- **WHEN** a PATCH request is made to `/api/config` with `{ "pii": { "tiers": { "slm": true } } }`
- **AND** either `pii.tiers.regex` or `pii.tiers.ner` is `false`
- **THEN** a 422 Unprocessable Entity response is returned with `"error": "Tier 3 requires Tier 1 and Tier 2"`

#### Scenario: Port change returns restart_required

- **WHEN** a PATCH request is made to `/api/config` with `{ "proxy": { "listen": "127.0.0.1:9090" } }`
- **THEN** a 200 response is returned with `{ "ok": true, "restart_required": true }`
- **AND** the change is persisted to `config.toml` but takes effect only after restart

### Requirement: Proxy Start/Stop API

The dashboard server SHALL expose `POST /api/proxy/stop` and `POST /api/proxy/start` endpoints. `stop` SHALL gracefully terminate the proxy listener (existing connections drained, no new connections accepted) and broadcast a `proxy_status` WebSocket event with `{ "running": false }`. `start` SHALL resume the proxy listener and broadcast `{ "running": true }`. `GET /api/proxy/status` SHALL return the current state. The proxy SHALL write its PID to `~/.config/privacyclaw/privacyclaw.pid` on startup and remove it on clean shutdown.

#### Scenario: Stop via API

- **WHEN** `POST /api/proxy/stop` is called
- **THEN** the proxy stops accepting new connections
- **AND** a `proxy_status` WebSocket event is broadcast with `{ "running": false }`
- **AND** the PID file is removed

#### Scenario: Start via API

- **WHEN** the proxy listener is stopped and `POST /api/proxy/start` is called
- **THEN** the listener resumes on port 16440
- **AND** a `proxy_status` WebSocket event is broadcast with `{ "running": true }`

#### Scenario: Status reflects current state

- **WHEN** `GET /api/proxy/status` is called
- **THEN** `{ "running": true|false, "http_proxy": true|false, "network_proxy": true|false, "pii_mode": "off|detect-only|replace" }` is returned

### Requirement: Dashboard Configuration UI Panel

The dashboard SHALL include a Settings panel accessible via a gear icon or dedicated tab. The panel SHALL render live toggles for:

- Proxy on/off (Start / Stop button with status indicator: green = running, red = stopped)
- HTTP Proxy enabled/disabled
- Network Proxy enabled/disabled (triggers privilege escalation flow when enabling)
- PII Mode selector: Off / Detect-only / Replace
- PII Tier 1 (regex) toggle — always enabled when PII mode is active
- PII Tier 2 (NER) toggle — only enabled when Tier 1 is on
- PII Tier 3 (SLM) toggle — only enabled when Tier 1 and Tier 2 are on

Toggling a tier that depends on a disabled parent SHALL automatically enable all required parents before enabling the selected tier. A `restart_required` banner SHALL appear when a port change is pending.

#### Scenario: Settings panel opens

- **WHEN** the user clicks the gear icon in the dashboard header
- **THEN** the Settings panel slides into view showing current values fetched from `GET /api/config`

#### Scenario: PII Tier 2 auto-enables Tier 1

- **WHEN** the user enables the Tier 2 toggle while Tier 1 is off
- **THEN** both Tier 1 and Tier 2 are toggled on in a single PATCH call
- **AND** both toggles reflect the new state without a page reload

#### Scenario: Restart required banner

- **WHEN** the server responds with `restart_required: true`
- **THEN** a yellow banner appears: "Port change saved — restart privacyclaw to apply"
- **AND** the banner is dismissible

### Requirement: Dashboard Model Management UI Panel

The dashboard SHALL include a Model Management section within the Settings panel showing the GGUF model catalog table. Each row SHALL display: model name, Q4 file size, RAM requirement, inference speed estimate, download/delete button, and an "Activate" button (enabled only when downloaded). The currently active model SHALL be highlighted. Download progress SHALL update in real time via WebSocket events.

#### Scenario: Model table rendered

- **WHEN** the Settings panel opens
- **THEN** all four catalog models are listed with their metadata and download/activation state from `GET /api/models`

#### Scenario: Download progress live update

- **WHEN** a model download is in progress
- **THEN** the row's download button shows a progress percentage that updates as `model_download_progress` WebSocket events arrive

#### Scenario: Activate button enabled after download

- **WHEN** a model download completes successfully
- **THEN** the Activate button for that row becomes clickable and the download button changes to Delete

#### Scenario: Active model highlighted

- **WHEN** a model is active
- **THEN** its row is visually highlighted and a badge shows "Active (Tier 3)"
