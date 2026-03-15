# llm-parser Specification

## Purpose
TBD - created by archiving change add-privacyclaw-mvp. Update Purpose after archive.
## Requirements
### Requirement: Provider Detection

The parser SHALL infer the LLM provider from the request's target domain.

#### Scenario: Anthropic provider detected

- **WHEN** the request target domain is `api.anthropic.com`
- **THEN** the provider is set to `"anthropic"`

#### Scenario: OpenAI provider detected

- **WHEN** the request target domain is `api.openai.com`
- **THEN** the provider is set to `"openai"`

#### Scenario: Google provider detected

- **WHEN** the request target domain is `generativelanguage.googleapis.com`
- **THEN** the provider is set to `"google"`

### Requirement: LLM Request Parsing

The parser SHALL extract structured data from LLM API request bodies: provider, model name, messages array (system, user, assistant, tool), tool definitions, and metadata.

#### Scenario: Anthropic messages request parsed

- **WHEN** a POST to `/v1/messages` with a JSON body containing `model` and `messages` is intercepted
- **THEN** the model name and messages array are extracted and stored

#### Scenario: OpenAI chat completions request parsed

- **WHEN** a POST to `/v1/chat/completions` with a JSON body containing `model` and `messages` is intercepted
- **THEN** the model name and messages array are extracted and stored

#### Scenario: Malformed request body

- **WHEN** the request body is not valid JSON or missing expected fields
- **THEN** the parser logs a warning and stores the raw bytes without panicking

### Requirement: Non-Streaming Response Parsing

The parser SHALL parse standard JSON responses from LLM APIs and extract the full response content.

#### Scenario: Anthropic non-streaming response

- **WHEN** the response Content-Type is `application/json` and the body contains an Anthropic messages response
- **THEN** the content blocks are extracted and stored as the assistant message

#### Scenario: OpenAI non-streaming response

- **WHEN** the response Content-Type is `application/json` and the body contains an OpenAI chat completion
- **THEN** the choices array is parsed and the assistant message content is extracted

### Requirement: SSE Streaming Response Parsing

The parser SHALL handle Server-Sent Events (SSE) streaming responses, buffering partial chunks, extracting text deltas, and accumulating the full response.

#### Scenario: Anthropic SSE stream parsed

- **WHEN** the response Content-Type is `text/event-stream` from `api.anthropic.com`
- **THEN** each `content_block_delta` event's `text_delta.text` is extracted and accumulated
- **AND** each delta is broadcast to connected WebSocket clients

#### Scenario: OpenAI SSE stream parsed

- **WHEN** the response Content-Type is `text/event-stream` from `api.openai.com`
- **THEN** each `data:` line's `choices[0].delta.content` is extracted and accumulated
- **AND** the `data: [DONE]` sentinel terminates accumulation

#### Scenario: Partial SSE chunk buffered

- **WHEN** a TCP segment arrives containing an incomplete SSE event
- **THEN** the partial data is buffered and combined with subsequent segments before parsing

#### Scenario: SSE accumulation buffer overflow

- **WHEN** accumulated SSE data for a single response exceeds 10MB
- **THEN** accumulation stops but forwarding to the client continues uninterrupted

### Requirement: Read-Only Passthrough

The parser SHALL operate on a copy of the byte stream. The original bytes forwarded to the client MUST be bit-identical to what the upstream server sent.

#### Scenario: Response byte identity

- **WHEN** the parser processes any response
- **THEN** the bytes received by the client are identical to what the upstream server sent
- **AND** no header, body, or framing bytes are modified

### Requirement: Anthropic SSE Stream Termination Detection
The Anthropic SSE parser SHALL recognize `message_stop` as the stream terminator and return `true` from `process_response_chunk`, triggering `finalize_response`.

#### Scenario: message_stop triggers finalization
- **WHEN** an SSE chunk contains `event: message_stop`
- **THEN** `process_response_chunk` returns `true`
- **AND** `finalize_response` is called and the response is stored

#### Scenario: Stream does not terminate early
- **WHEN** SSE events arrive that are not `message_stop` or `[DONE]`
- **THEN** `process_response_chunk` returns `false` and streaming continues

### Requirement: Token Count Extraction from SSE Events
The Anthropic parser SHALL extract `input_tokens` from `message_start` events and `output_tokens` from `message_delta` events, storing them on the response message.

#### Scenario: Input tokens extracted
- **WHEN** a `message_start` event contains `usage.input_tokens: 100`
- **THEN** the stored response message has `tokens_in = Some(100)`

#### Scenario: Output tokens extracted
- **WHEN** a `message_delta` event contains `usage.output_tokens: 50`
- **THEN** the stored response message has `tokens_out = Some(50)`

### Requirement: Malformed Input Handling
All parsers SHALL return `None` (not panic) when given malformed, empty, or unexpected JSON input.

#### Scenario: Malformed JSON body
- **WHEN** `parse_request` receives bytes that are not valid JSON
- **THEN** it returns `None` without panicking

#### Scenario: Missing required fields
- **WHEN** `parse_request` receives valid JSON that lacks the `messages` field
- **THEN** it returns `None`

### Requirement: Multi-Turn Parse Performance
`parse_request` for Anthropic SHALL parse a 182-message, ~600 KB request body in under 50 ms on standard development hardware.

#### Scenario: Large context parsed within time budget
- **WHEN** `parse_request` is called with a 182-message Anthropic request body
- **THEN** it completes in under 50 ms and returns the correct message count

