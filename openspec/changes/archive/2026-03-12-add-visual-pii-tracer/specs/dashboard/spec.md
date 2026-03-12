## MODIFIED Requirements

### Requirement: PII Detection Dashboard Panel

The `WsEvent::Message` variant SHALL include `content_masked: Option<String>` and `pii_processed: Option<bool>` so live-streaming clients receive the replaced content without waiting for a page reload.

The vault API endpoint `GET /api/conversations/:id/vault` SHALL include `"confidence"` in each entry.

#### Scenario: WsEvent::Message carries masked content

- **WHEN** a request message is processed with PII replacement active
- **THEN** the `WsEvent::Message` event includes `content_masked` with the replaced text
- **AND** existing clients that do not read `content_masked` are unaffected (optional field)

#### Scenario: Vault API includes confidence

- **WHEN** `GET /api/conversations/:id/vault` is called for a conversation with detections
- **THEN** each entry includes `"confidence": 0.95` (or `null` for legacy records)

---

## ADDED Requirements

### Requirement: Compare View Column 2 Accuracy

Column 2 ("Sent to LLM") of the compare view SHALL use the server-stored `content_masked` value when available, rather than a client-side reconstruction.

#### Scenario: Stored content_masked used for column 2

- **WHEN** a request message has `content_masked` set (pipeline ran, replacements made)
- **THEN** column 2 displays the stored replaced text exactly as forwarded to the LLM

#### Scenario: Fallback to client-side approximation for legacy messages

- **WHEN** a request message has `pii_processed: null` (legacy, predates this feature)
- **THEN** column 2 displays a client-side approximation via `applyPiiMasking`
- **AND** an amber `(approx)` badge is shown on the column 2 bubble
- **AND** the badge tooltip reads "Stored before server-side masking was recorded; showing client-side approximation"

#### Scenario: No masking when pipeline found nothing

- **WHEN** a request message has `pii_processed: false` and `content_masked: null`
- **THEN** column 2 displays the same text as column 1 with no badge

---

### Requirement: Per-Column PII Span Highlighting

Each column of the compare view SHALL use a distinct visual treatment for PII spans.

#### Scenario: Column 1 highlights original PII locations

- **WHEN** the compare view renders column 1 (Original Request)
- **THEN** substrings matching vault `original_masked` labels are highlighted with an amber dashed underline
- **AND** hovering shows a tooltip: `"[ENTITY_TYPE] — replaced with §synthetic§"`

#### Scenario: Column 2 and 3 highlight synthetic tokens

- **WHEN** the compare view renders column 2 (Sent to LLM) or column 3 (LLM Response)
- **THEN** substrings matching vault synthetic values are highlighted with a blue solid underline
- **AND** hovering shows a tooltip: `"synthetic for: [original_masked]"`

#### Scenario: Column 4 highlights restored values

- **WHEN** the compare view renders column 4 (Delivered to User)
- **THEN** substrings matching vault original values are highlighted with a green solid underline
- **AND** hovering shows a tooltip: `"restored from synthetic §token§"`

---

### Requirement: Per-Message Detection Sidebar

The dashboard SHALL display a detection sidebar when the user clicks any message bubble in the compare view. The sidebar shows all PII detections attributed to that specific message turn.

#### Scenario: Sidebar opens on bubble click

- **WHEN** the user clicks a message bubble in the compare view
- **THEN** the sidebar opens on the right side of the view
- **AND** a `GET /api/conversations/:id/detections?message_id=:msg_id` request is made
- **AND** the sidebar renders a table: Entity Type | Original (masked) | Synthetic | Tier | Confidence

#### Scenario: Confidence bar displayed

- **WHEN** a detection has `confidence` > 0
- **THEN** a CSS `<progress>` bar (0–1 range) is shown in the confidence column

#### Scenario: Legacy attribution unavailable

- **WHEN** a detection record has no `message_id` (legacy conversation)
- **THEN** the sidebar shows all vault entries for the conversation with a note: "No per-message attribution available for this session"

#### Scenario: No detections for message

- **WHEN** the detection endpoint returns an empty array for a message
- **THEN** the sidebar shows "No PII detected in this message"

---

### Requirement: Detections API Endpoint

The dashboard HTTP server SHALL expose `GET /api/conversations/:id/detections` with an optional `?message_id=` query parameter.

#### Scenario: Filtered by message_id

- **WHEN** `GET /api/conversations/abc/detections?message_id=msg-123` is called
- **THEN** only detection records with `message_id == "msg-123"` are returned
- **AND** the response is a JSON array of `{entity_type, original_masked, synthetic, tier, confidence}`

#### Scenario: All detections for conversation

- **WHEN** `GET /api/conversations/abc/detections` is called without `message_id`
- **THEN** all detection records for the conversation are returned in file order

---

### Requirement: Turn Navigator

The compare view SHALL display a compact turn navigator bar above the message list. Each turn is represented by a numbered chip. Turns with PII detections show a red badge with the detection count.

#### Scenario: Turn chips rendered

- **WHEN** the compare view loads a conversation with N request/response pairs
- **THEN** N numbered chips are rendered in the turn navigator bar
- **AND** chips with detections show a red badge with the count

#### Scenario: Chip click scrolls to turn

- **WHEN** the user clicks chip N
- **THEN** the compare view scrolls smoothly to the first message row of turn N

#### Scenario: Live turn chip appended

- **WHEN** a `WsEvent::Message` with `direction: "request"` arrives during an active session
- **THEN** a new turn chip is appended to the navigator

#### Scenario: Detection badge incremented live

- **WHEN** a `WsEvent::PiiDetected` event arrives for the current turn
- **THEN** the chip badge count increments by 1

---

### Requirement: Conversation Summary Bar

The compare view SHALL display a one-line summary bar at the top showing total PII entity count, breakdown by tier, and breakdown by entity type.

#### Scenario: Summary computed from vault

- **WHEN** the compare view loads
- **THEN** the summary bar shows: `Total PII: N | T1: x  T2: y  T3: z | EMAIL: a  NAME: b  ...`
- **AND** the data is computed from the in-memory vault (no additional API call)

#### Scenario: Summary updates live

- **WHEN** a `WsEvent::PiiDetected` event arrives
- **THEN** the summary bar increments the relevant tier and type counters

---

### Requirement: Conversation List Limit

The `GET /api/conversations` endpoint SHALL support a `?limit=N` query parameter (default 50, maximum 200) replacing the previous hardcoded limit of 10.

#### Scenario: Default limit returns 50

- **WHEN** `GET /api/conversations` is called without a limit parameter
- **THEN** up to 50 conversations are returned, most recent first

#### Scenario: Custom limit respected

- **WHEN** `GET /api/conversations?limit=20` is called
- **THEN** up to 20 conversations are returned
