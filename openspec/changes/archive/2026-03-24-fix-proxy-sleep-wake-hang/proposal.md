# Change: Fix proxy stops serving after macOS sleep/wake

## Why

After macOS sleep/wake, the CONNECT proxy accept loop dies permanently (tray mode) or
connection handlers hang indefinitely (CLI mode). The root cause is three layered defects
in `proxy/mod.rs` and `proxy/connect.rs`: a fatal `?` propagation on per-connection accept
errors, missing timeouts on `read_line`, and missing timeouts on upstream TCP/TLS connect
and bidirectional copy operations.

## What Changes

- `src/proxy/mod.rs:37` — convert `listener.accept().await?` to `match`/`continue`,
  mirroring the already-correct pattern in `network.rs`; transient per-connection errors
  (ECONNABORTED, ECONNRESET from stale pre-sleep backlog) no longer kill the accept loop
- `src/proxy/connect.rs:28,35` — wrap both `read_line` calls with
  `tokio::time::timeout(30s, ...)` so connections that open a TCP socket but never send a
  CONNECT line are dropped after 30 seconds
- `src/proxy/connect.rs:62,94,103,69` — add explicit timeouts to upstream TCP connect
  (10 s), TLS handshake (10 s), and `copy_bidirectional` idle (300 s) so stale
  pre-sleep passthrough/MITM sockets do not block indefinitely after wake

## Impact

- Affected specs: `mitm-proxy`
- Affected code: `src/proxy/mod.rs`, `src/proxy/connect.rs`
- No breaking changes; behavior is unchanged for healthy connections
