# sse-parser Specification

## Purpose
TBD - created by archiving change add-test-coverage. Update Purpose after archive.
## Requirements
### Requirement: Line Ending Tolerance
The SSE parser SHALL correctly parse events delimited by LF-only (`\n`) line endings in addition to CRLF (`\r\n`).

#### Scenario: LF-only delimiters parsed correctly
- **WHEN** an SSE stream uses `\n` without `\r`
- **THEN** events are emitted identically to a CRLF-delimited stream

### Requirement: Comment and Retry Line Handling
The SSE parser SHALL silently ignore comment lines (starting with `:`) and `retry:` lines without emitting events or erroring.

#### Scenario: Comment lines produce no events
- **WHEN** the SSE stream contains lines starting with `:`
- **THEN** no events are emitted for those lines

### Requirement: Multi-Line Data Field Concatenation
When an SSE event contains multiple `data:` lines, the parser SHALL concatenate them with a newline character as specified by the SSE standard.

#### Scenario: Two data lines joined
- **WHEN** an event block has two `data:` lines: `data: hello` and `data: world`
- **THEN** the emitted event has `data == "hello\nworld"`

### Requirement: Incremental Chunk Stability
Feeding an SSE byte stream one byte at a time SHALL produce the same sequence of events as feeding the entire stream in a single push.

#### Scenario: Byte-by-byte equals full push
- **WHEN** an SSE stream is fed to `SseParser::push` one byte per call
- **THEN** the union of all returned event lists equals the events returned by a single push of the full stream

### Requirement: Large Stream Throughput
The SSE parser SHALL process a stream of 5000 events in under 100 ms.

#### Scenario: 5000-event stream parsed within time budget
- **WHEN** `SseParser::push` is called with a pre-built byte slice containing 5000 events
- **THEN** it completes in under 100 ms and returns exactly 5000 events (excluding bookend events)

