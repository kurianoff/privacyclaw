## ADDED Requirements

### Requirement: Intercept Pipeline Context Encapsulation

The intercept pipeline SHALL encapsulate the six shared state fields (`shared_conv_id`, `shared_vault`, `store`, `ws_tx`, `pii`, `provider`) into a single `InterceptContext` struct. No function in `src/proxy/intercept.rs` SHALL require more than five parameters. The `#[allow(clippy::too_many_arguments)]` attribute SHALL NOT appear anywhere in `intercept.rs`.

#### Scenario: Context passed to handler

- **WHEN** `intercept::run()` is called with the six shared fields
- **THEN** it constructs one `InterceptContext` value and passes it (by value or reference) to every internal handler function
- **AND** each handler function signature lists at most five parameters

#### Scenario: Clippy clean

- **WHEN** `cargo clippy -- -D warnings` is run on the workspace
- **THEN** no `too_many_arguments` suppression is needed in `intercept.rs`

---

### Requirement: HTTP Body Reader Abstraction

The intercept pipeline SHALL use a single `HttpBodyReader` struct to manage the chunk-accumulation state machine (`header_done`, `content_length`, `body_start`, `body_received`). This state machine SHALL NOT be duplicated across handler functions.

#### Scenario: Body complete detection

- **WHEN** chunks are pushed to `HttpBodyReader` until `Content-Length` bytes have been received
- **THEN** `push()` returns `true` exactly once (on the chunk that completes the body) and never again for that request

#### Scenario: Keep-alive reuse

- **WHEN** a new request arrives on a keep-alive connection
- **THEN** calling `reset()` on the existing `HttpBodyReader` reinitialises all state to initial values
- **AND** subsequent `push()` calls track the new request independently

#### Scenario: Single implementation

- **WHEN** the codebase is searched for the pattern `header_done`, `body_received`, `body_start` as struct fields or local variables
- **THEN** they appear only inside `HttpBodyReader` and its methods, not in any handler function body

---

### Requirement: Shared Passthrough Implementation

The bidirectional TCP copy-and-log logic SHALL exist in exactly one location (`src/proxy/passthrough.rs`). Both the CONNECT proxy path and the network proxy path SHALL call the same implementation.

#### Scenario: CONNECT passthrough delegates to shared function

- **WHEN** a non-allowlisted domain is connected via CONNECT proxy
- **THEN** `connect.rs` calls `proxy::passthrough::copy_bidirectional_logged()` and does not contain its own copy of the bidirectional copy loop

#### Scenario: Network passthrough delegates to shared function

- **WHEN** a non-allowlisted domain is connected via network proxy
- **THEN** `network.rs` calls `proxy::passthrough::copy_bidirectional_logged()` and does not contain its own copy of the bidirectional copy loop

---

### Requirement: Protocol Constant Naming

Magic numeric literals used in TLS record inspection and DNS packet construction/parsing in `src/proxy/network.rs` SHALL be replaced with named constants. No bare hex or numeric literals representing protocol fields SHALL appear in `peek_sni()`, `build_dns_a_query()`, or `parse_first_a_record()`.

#### Scenario: TLS handshake detection uses named constant

- **WHEN** `peek_sni()` checks the first byte of an incoming connection for a TLS ClientHello
- **THEN** it references `TLS_RECORD_TYPE_HANDSHAKE` (value `0x16`) rather than the literal `0x16`

#### Scenario: DNS A record type uses named constant

- **WHEN** `parse_first_a_record()` filters DNS response records by type
- **THEN** it references `DNS_A_RECORD_TYPE` (value `1`) rather than the literal `1`
