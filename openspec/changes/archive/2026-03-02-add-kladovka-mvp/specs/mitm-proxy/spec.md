## ADDED Requirements

### Requirement: HTTP CONNECT Proxy Listener

The proxy SHALL listen on a configurable address (default `127.0.0.1:8080`) and accept HTTP CONNECT tunnel requests from clients.

#### Scenario: CONNECT request accepted

- **WHEN** a client sends `CONNECT api.anthropic.com:443 HTTP/1.1`
- **THEN** the proxy responds with `200 Connection established`
- **AND** proceeds to route based on domain allowlist

#### Scenario: Malformed CONNECT request

- **WHEN** the proxy receives a malformed or non-CONNECT HTTP request
- **THEN** it responds with an appropriate HTTP error and closes the connection gracefully without panicking

### Requirement: Domain-Based Routing

The proxy SHALL route CONNECT requests based on a configurable domain allowlist. Allowlisted domains are MITMed; all others are passed through transparently.

#### Scenario: Allowlisted domain intercepted

- **WHEN** the target domain is in the `[intercept].domains` configuration list
- **THEN** the proxy performs TLS MITM interception

#### Scenario: Non-allowlisted domain passed through

- **WHEN** the target domain is not in the allowlist
- **THEN** the proxy establishes a raw TCP tunnel without TLS termination
- **AND** the original server certificate is presented to the client unchanged

### Requirement: TLS MITM Interception

For allowlisted domains, the proxy SHALL terminate TLS with the client (using a dynamically generated leaf cert) and establish a new TLS connection to the upstream server.

#### Scenario: Full MITM handshake

- **WHEN** intercepting an allowlisted domain
- **THEN** the proxy presents a leaf cert signed by the local CA to the client
- **AND** establishes a TLS connection to the real upstream using webpki-roots for verification
- **AND** forwards decrypted traffic bidirectionally

### Requirement: Zero-Latency Passthrough

The proxy SHALL forward bytes to the client as they arrive from upstream, with parsing occurring on a tee'd copy on a separate channel. The proxy MUST NOT buffer bytes on the critical forwarding path.

#### Scenario: Streaming response forwarding

- **WHEN** an upstream LLM API sends SSE chunks
- **THEN** each chunk is forwarded to the client immediately
- **AND** a copy is sent to the parser channel concurrently without blocking the forward path

### Requirement: Transparent TCP Passthrough

For non-allowlisted domains, the proxy SHALL establish a raw TCP tunnel with no TLS inspection.

#### Scenario: Non-LLM HTTPS passthrough

- **WHEN** a client connects to `github.com:443` via the proxy
- **THEN** a raw TCP tunnel is established to `github.com:443`
- **AND** the client receives the original GitHub TLS certificate
- **AND** no traffic content is inspected or logged
