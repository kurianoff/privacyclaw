## ADDED Requirements

### Requirement: HTTP Dashboard Server

The system SHALL serve a single-page web dashboard on a configurable address (default `127.0.0.1:8443`) using embedded static assets (HTML, CSS, JS) compiled into the binary.

#### Scenario: Dashboard served

- **WHEN** a browser requests `http://localhost:8443/`
- **THEN** the embedded `index.html` is returned with 200 OK
- **AND** no external network requests are needed to render the page

#### Scenario: REST conversation list

- **WHEN** a GET request is made to `/api/conversations`
- **THEN** a JSON array of conversations is returned, most recent first

#### Scenario: REST conversation detail

- **WHEN** a GET request is made to `/api/conversations/:id`
- **THEN** a JSON object with the conversation and all messages is returned

### Requirement: WebSocket Real-Time Streaming

The dashboard server SHALL maintain a WebSocket endpoint at `/ws` and broadcast live events to all connected clients as LLM traffic is intercepted.

#### Scenario: Conversation start broadcast

- **WHEN** a new conversation begins
- **THEN** a `conversation_start` message is broadcast to all WebSocket clients with id, provider, model, timestamp

#### Scenario: SSE text delta broadcast

- **WHEN** a streaming SSE text delta arrives
- **THEN** a `text_delta` message is broadcast with conversation_id, text, timestamp

#### Scenario: Response complete broadcast

- **WHEN** an LLM response finishes streaming
- **THEN** a `response_complete` message is broadcast with conversation_id, tokens_in, tokens_out

### Requirement: Dashboard UI — Conversation List

The dashboard HTML/JS SHALL display a left panel listing intercepted conversations most recent first, with provider name, model, message count, timestamp, and a green live indicator for active streaming conversations.

#### Scenario: Conversation list displayed

- **WHEN** the page loads
- **THEN** existing conversations are fetched via REST and rendered in the left panel

#### Scenario: New conversation appears live

- **WHEN** a `conversation_start` WebSocket event is received
- **THEN** the new conversation is prepended to the list without a page reload

### Requirement: Dashboard UI — Conversation Detail

The dashboard SHALL display the selected conversation in a chat-style main panel with system prompt in a collapsible section, user/assistant messages styled differently, live streaming tokens appearing in real time with a blinking cursor, and metadata (model, token counts, latency, timestamps).

#### Scenario: Live token streaming in UI

- **WHEN** the selected conversation is actively streaming
- **THEN** each `text_delta` WebSocket message appends text to the current assistant message
- **AND** a blinking cursor is shown during streaming
