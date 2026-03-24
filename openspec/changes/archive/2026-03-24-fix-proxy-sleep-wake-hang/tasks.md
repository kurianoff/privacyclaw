## 1. Fix accept loop fatal error propagation (proxy/mod.rs)

- [x] 1.1 In `src/proxy/mod.rs`, replace `let (stream, peer_addr) = listener.accept().await?;`
  with a `match` that logs a WARN and `continue`s on error, mirroring the pattern at
  `network.rs:71-86`. Add a `tokio::time::sleep(Duration::from_millis(10))` before
  `continue` to avoid a tight error loop under sustained accept failures.
  Verification: `cargo build` passes; `cargo clippy -- -D warnings` clean.

## 2. Add read_line timeout in connect handler (connect.rs)

- [x] 2.1 In `src/proxy/connect.rs`, wrap the CONNECT request-line `read_line` at line 28
  with `tokio::time::timeout(Duration::from_secs(30), ...)`. Map timeout expiry to an
  informational WARN log and return early (`return Ok(())`).
  Verification: `cargo build` passes.

- [x] 2.2 In `src/proxy/connect.rs`, wrap the header-drain `read_line` at line 35
  (inside the loop) with the same 30-second timeout. Map timeout expiry to WARN + early
  return.
  Verification: `cargo build` passes; `cargo clippy -- -D warnings` clean.

## 3. Add upstream connect, TLS, and copy timeouts (connect.rs)

- [x] 3.1 In `src/proxy/connect.rs::passthrough`, wrap `TcpStream::connect(&addr)` at
  line 62 with `tokio::time::timeout(Duration::from_secs(10), ...)`. Return an error on
  timeout with a descriptive message.
  Verification: `cargo build` passes.

- [x] 3.2 In `src/proxy/connect.rs::passthrough`, replace
  `tokio::io::copy_bidirectional(&mut stream, &mut upstream)` at line 69 with a
  `tokio::time::timeout(Duration::from_secs(300), ...)` wrapper. On timeout, log WARN
  ("passthrough idle timeout") and return `Ok(())` (not an error — idle close is normal).
  Verification: `cargo build` passes.

- [x] 3.3 In `src/proxy/connect.rs::mitm`, wrap `TcpStream::connect(addr)` at line 94
  with `tokio::time::timeout(Duration::from_secs(10), ...)`. Return an error on timeout.
  Verification: `cargo build` passes.

- [x] 3.4 In `src/proxy/connect.rs::mitm`, wrap `connector.connect(server_name, upstream_tcp)`
  at line 103 with `tokio::time::timeout(Duration::from_secs(10), ...)`. Return an error
  on timeout.
  Verification: `cargo build` passes; `cargo clippy -- -D warnings` clean; full
  `cargo test` passes (373 lib tests; 2 pre-existing brew formula tests unrelated to fix).
