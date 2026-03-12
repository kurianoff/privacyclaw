## ADDED Requirements

### Requirement: PII Detection Dashboard Panel

The dashboard SHALL display PII detection results for each conversation when PII mode is active. The panel SHALL show a visual diff between the original message content and the sanitised version sent to the LLM, along with the vault mapping table for that conversation.

#### Scenario: PII detection indicator on conversation list

- **WHEN** a conversation was processed with PII mode enabled and entities were detected
- **THEN** the conversation list entry shows a shield icon or "PII" badge
- **AND** the count of detected entities is displayed

#### Scenario: PII diff panel in conversation detail

- **WHEN** the user selects a conversation with PII detections
- **THEN** a collapsible "Privacy" panel is shown below each affected message
- **AND** the panel shows original text (redacted for display: `***email***`) and synthetic value side by side

#### Scenario: Vault mapping table

- **WHEN** the user opens the "Vault" tab for a conversation
- **THEN** a table is displayed with columns: Type, Original (masked), Synthetic, Tier, Detected at
- **AND** original values are masked as `[EMAIL]`, `[NAME]`, etc. — NOT shown in plaintext in the UI

#### Scenario: Live PII event during streaming

- **WHEN** the PII pipeline detects an entity during active request processing
- **THEN** a `WsEvent::PiiDetected` message is broadcast to connected dashboard clients
- **AND** the entity appears in the vault table in real time

---

### Requirement: PII REST Endpoint

The dashboard HTTP server SHALL expose a `GET /api/conversations/:id/vault` endpoint that returns the vault mappings for a conversation. Original values SHALL be masked in the API response.

#### Scenario: Vault endpoint returns masked originals

- **WHEN** `GET /api/conversations/abc123/vault` is called
- **THEN** the response is a JSON array of `{type, original_masked, synthetic, tier, confidence}`
- **AND** `original_masked` is e.g. `"[EMAIL]"` not `"john@acme.com"`

#### Scenario: No vault for conversation

- **WHEN** the conversation has no vault (no PII detected or PII mode was off)
- **THEN** the endpoint returns an empty array `[]` with status 200
