# Change: Phased maintainability refactoring

## Why

Six functions in `intercept.rs` take 9–10 parameters each (suppressed with `#[allow(clippy::too_many_arguments)]`), and the HTTP body-reading state machine is copy-pasted three times in the same file. The storage module scans the log directory on every message write (O(N) per write). Together these patterns make `intercept.rs` (888 lines) and `storage/mod.rs` (787 lines) the two hardest files to change safely. The refactoring has no user-visible behavior change — all existing tests continue to pass unmodified.

## What Changes

### Phase 1 — Intercept pipeline context and HTTP body reader (`intercept.rs`)
- Extract `InterceptContext` struct holding the 6 fields shared across every function (`shared_conv_id`, `shared_vault`, `store`, `ws_tx`, `pii`, `provider`) — removes `#[allow(clippy::too_many_arguments)]` from all six suppressed functions
- Extract `HttpBodyReader` struct encapsulating the chunk-reading state machine (`header_done`, `content_length`, `body_received`, `body_start`) — eliminates ~90 lines of copy-pasted state across `handle_c2u_passthrough`, `handle_c2u_pii`, and `handle_u2c`

### Phase 2 — Storage path-lookup cache (`storage/mod.rs`)
- Add `conv_cache: Arc<RwLock<HashMap<String, PathBuf>>>` to `Store` — populated once at `Store::open()` and invalidated on `rotate_old()` — making `conv_file_path()` O(1) instead of O(N directory scan) per write

### Phase 3 — Cross-module deduplication (`main.rs`, `proxy/`, `parser/`)
- Merge `passthrough()` (connect.rs) and `passthrough_raw()` (network.rs) into a single shared `proxy::passthrough::copy_bidirectional_logged()` — eliminates drift risk between the two copies
- Extract shared `cmd_start` / `cmd_network_start` setup (~97% identical) into `fn build_proxy_runtime()` in main.rs — reduces ~174 lines to ~60
- Add `ProviderParser` trait to parser module so `parse_request()` and `extract_sse_delta()` dispatch through a trait instead of repeating `match provider { … }` blocks

### Phase 4 — Polish (`network.rs`, `ca/`, `config.rs`)
- Replace magic hex literals in `network.rs` SNI/DNS parsing with named constants (`TLS_RECORD_TYPE_HANDSHAKE`, `CLIENT_HELLO_TYPE`, `DNS_COMPRESSION_FLAG`, `SNI_EXT_TYPE`, `DNS_A_RECORD_TYPE`)
- Remove never-read `cert_pem` field from `CaBundle`; deduplicate CA Distinguished Name params shared between `ca/mod.rs` and `cert_gen.rs` into a single module-level const
- Rename `SingleKeyResolver` → `StaticKeyResolver` in `cert_gen.rs` (the existing name implies count, not behavior)
- Add `Config::validate()` checking port ranges (1–65535), positive timeout values, and non-empty log directory path; call from `Config::load()` after parsing

## Impact

- Affected specs: `mitm-proxy`, `storage`
- Affected code: `src/proxy/intercept.rs`, `src/storage/mod.rs`, `src/main.rs`, `src/proxy/connect.rs`, `src/proxy/network.rs`, `src/parser/mod.rs`, `src/ca/mod.rs`, `src/ca/cert_gen.rs`, `src/config.rs`
- No behavior changes — all existing 175 tests must pass after every phase
- No new external dependencies
