## ADDED Requirements

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
