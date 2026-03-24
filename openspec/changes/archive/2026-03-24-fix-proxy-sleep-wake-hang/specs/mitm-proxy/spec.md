## ADDED Requirements

### Requirement: Accept Loop Error Resilience
The CONNECT proxy accept loop SHALL continue running after transient per-connection
accept errors (e.g., ECONNABORTED, ECONNRESET). A WARN SHALL be logged per error; the
loop SHALL NOT exit. A brief sleep (10 ms) SHALL follow each error to prevent a tight
error-retry loop.

#### Scenario: ECONNABORTED from stale pre-sleep backlog
- **WHEN** `listener.accept()` returns an OS-level connection error (e.g., ECONNABORTED
  after macOS wake)
- **THEN** the error is logged at WARN level
- **AND** the accept loop continues without rebinding the listener
- **AND** the next `accept()` call proceeds normally

#### Scenario: Sustained accept errors
- **WHEN** `listener.accept()` returns errors on multiple consecutive calls
- **THEN** each error is logged individually at WARN level with a 10 ms sleep between
  retries
- **AND** the accept loop does not exit and does not consume unbounded CPU

### Requirement: CONNECT Request Read Timeout
The CONNECT proxy connection handler SHALL apply a 30-second timeout to reading the
CONNECT request line and draining HTTP headers. Connections that open a TCP socket but
never complete the CONNECT handshake SHALL be dropped after 30 seconds with a WARN log.

#### Scenario: Client opens TCP but never sends CONNECT line
- **WHEN** a client opens a TCP connection to the proxy
- **AND** no CONNECT request line arrives within 30 seconds
- **THEN** the handler logs WARN ("CONNECT read timeout") and returns without error
- **AND** the connection is closed

#### Scenario: CONNECT request arrives within 30 seconds
- **WHEN** the CONNECT request line arrives within the timeout window
- **THEN** the handler proceeds normally with no observable change

### Requirement: Upstream TCP Connect Timeout
The CONNECT proxy SHALL apply a 10-second timeout to `TcpStream::connect` for both
passthrough and MITM upstream connections. Connections that do not complete within 10
seconds SHALL return an error that is logged at WARN by the per-connection error handler.

#### Scenario: Upstream TCP connect times out
- **WHEN** the upstream host is unreachable and `TcpStream::connect` does not complete
  within 10 seconds
- **THEN** the handler returns an error
- **AND** the client connection is closed

#### Scenario: Upstream TCP connect succeeds within 10 seconds
- **WHEN** the upstream host responds before the 10-second timeout
- **THEN** the connection proceeds normally

### Requirement: Upstream TLS Handshake Timeout
The CONNECT proxy SHALL apply a 10-second timeout to the upstream TLS handshake on the
MITM path. A handshake that does not complete within 10 seconds SHALL return an error.

#### Scenario: TLS handshake times out after wake
- **WHEN** the upstream TLS handshake does not complete within 10 seconds
- **THEN** the handler returns an error and the session closes

#### Scenario: TLS handshake completes normally
- **WHEN** the TLS handshake completes within the timeout window
- **THEN** the MITM session proceeds normally

### Requirement: Passthrough Idle Timeout
The CONNECT proxy passthrough path SHALL apply a 300-second idle timeout to
`copy_bidirectional`. When the timeout fires, the handler SHALL log WARN
("passthrough idle timeout") and return `Ok(())` (normal close, not an error).

#### Scenario: Passthrough connection idles for 300 seconds
- **WHEN** no bytes flow through a passthrough TCP tunnel for 300 seconds
- **THEN** the handler logs WARN ("passthrough idle timeout") and returns `Ok(())`
- **AND** the connection is closed cleanly

#### Scenario: Active passthrough connection not affected
- **WHEN** bytes are flowing through a passthrough connection within the 300-second window
- **THEN** the copy continues uninterrupted
