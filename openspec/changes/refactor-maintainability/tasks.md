# Tasks: Phased maintainability refactoring

Each phase is independently mergeable. Run `cargo build && cargo test && cargo clippy -- -D warnings` after every checked item.

---

## Phase 1 — Intercept pipeline refactoring (`intercept.rs`)

**Goal**: eliminate all `#[allow(clippy::too_many_arguments)]` suppressions and the copy-pasted HTTP body state machine.

### 1.1 Define `HttpBodyReader` struct
- [ ] 1.1.1 Add `HttpBodyReader { header_done, content_length, body_start, body_received }` with `new()`, `push(&[u8]) -> bool`, `body<'a>(&self, raw: &'a [u8]) -> &'a [u8]`, and `reset()` methods in `intercept.rs` (above the handler functions)
- [ ] 1.1.2 Replace the inline state machine in `handle_c2u_passthrough` with `HttpBodyReader` — verify logic is equivalent
- [ ] 1.1.3 Replace the inline state machine in `handle_c2u_pii` with `HttpBodyReader`
- [ ] 1.1.4 Replace the inline state machine in `handle_u2c` with `HttpBodyReader`
- [ ] 1.1.5 Run `cargo test` — all tests pass; `cargo clippy -- -D warnings` — clean

### 1.2 Define `InterceptContext` struct
- [ ] 1.2.1 Add `struct InterceptContext { shared_conv_id, shared_vault, store, ws_tx, pii, provider }` in `intercept.rs`; derive/implement `Clone` (all fields are `Arc`-wrapped or `Copy`)
- [ ] 1.2.2 Update `handle_c2u` to take `ctx: InterceptContext` instead of 6 individual params; update its internal dispatches to `handle_c2u_passthrough` / `handle_c2u_pii` to pass `&ctx`
- [ ] 1.2.3 Update `handle_c2u_passthrough` signature to `(reader, writer, ctx: &InterceptContext, shutdown)`
- [ ] 1.2.4 Update `handle_c2u_pii` signature to `(reader, writer, ctx: &InterceptContext, shutdown)`
- [ ] 1.2.5 Update `handle_u2c` signature to `(reader, writer, ctx: InterceptContext, shutdown)` (takes ownership for async task)
- [ ] 1.2.6 Update `flush_rep_buf_and_finalize` and `finalize_response` to accept `ctx: &InterceptContext`
- [ ] 1.2.7 Update `process_sse_chunk_pii` to accept `ctx: &InterceptContext` in place of individual fields
- [ ] 1.2.8 Update `run()` (the public entry point) to construct `InterceptContext` and pass it to `handle_c2u` / `handle_u2c`
- [ ] 1.2.9 Remove all `#[allow(clippy::too_many_arguments)]` attributes from `intercept.rs`
- [ ] 1.2.10 Run `cargo build && cargo test && cargo clippy -- -D warnings` — all green

**Dependencies**: none
**Parallelizable**: 1.1 and 1.2 can be worked in sequence within Phase 1; 1.1 first since it reduces line count before the bigger restructure.

---

## Phase 2 — Storage path-lookup cache (`storage/mod.rs`)

**Goal**: make `conv_file_path()` O(1) instead of O(N directory scan) per message write.

### 2.1 Add cache to `Store`
- [ ] 2.1.1 Add `conv_cache: Arc<RwLock<HashMap<String, PathBuf>>>` field to the inner `StoreInner` struct (or equivalent internal struct) inside `storage/mod.rs`
- [ ] 2.1.2 In `Store::open()`, after opening the directory, scan existing conversation files and populate `conv_cache` with `(conv_id → path)` entries
- [ ] 2.1.3 Refactor `conv_file_path(&self, conv_id: &str) -> Result<PathBuf>` to read from cache first; fall back to directory scan only if not found (for robustness during cache cold-start edge case)
- [ ] 2.1.4 Update `insert_conversation()` to insert the new path into `conv_cache` at the same time as writing the file
- [ ] 2.1.5 Update `rotate_old()` to remove evicted conversation IDs from `conv_cache`

### 2.2 Validate
- [ ] 2.2.1 Run `cargo test` — all storage tests pass (including concurrent insert test)
- [ ] 2.2.2 Run `cargo clippy -- -D warnings` — clean
- [ ] 2.2.3 Manually verify: start proxy, send 3 requests, confirm no regression in log output

**Dependencies**: none — storage module is independent of Phase 1
**Parallelizable**: can be worked in parallel with Phase 1 by a second contributor

---

## Phase 3 — Cross-module deduplication (`main.rs`, `proxy/`, `parser/`)

**Goal**: eliminate three pockets of copy-pasted logic in main.rs, proxy passthrough, and parser dispatch.

### 3.1 Shared passthrough function
- [ ] 3.1.1 Create `src/proxy/passthrough.rs` with `pub async fn copy_bidirectional_logged(client: TcpStream, upstream: TcpStream, host: &str) -> Result<()>` — migrate the shared body from `connect.rs::passthrough()` and `network.rs::passthrough_raw()`
- [ ] 3.1.2 Add `pub mod passthrough;` to `src/proxy/mod.rs`
- [ ] 3.1.3 Replace `connect.rs::passthrough()` call site with `proxy::passthrough::copy_bidirectional_logged()`
- [ ] 3.1.4 Replace `network.rs::passthrough_raw()` call site with `proxy::passthrough::copy_bidirectional_logged()`
- [ ] 3.1.5 Delete the old local implementations from both files
- [ ] 3.1.6 Run `cargo test && cargo clippy -- -D warnings`

### 3.2 Merge `cmd_start` / `cmd_network_start` setup in `main.rs`
- [ ] 3.2.1 Extract `fn build_proxy_runtime(cfg: &Config) -> Result<ProxyRuntime>` returning a struct `{ cert_cache, store, ws_tx, pii_ctx, dashboard_task, rotation_task }` — contains the shared ~100 lines of setup
- [ ] 3.2.2 Update `cmd_start()` to call `build_proxy_runtime()` and then spawn only the CONNECT listener task
- [ ] 3.2.3 Update `cmd_network_start()` to call `build_proxy_runtime()` and then spawn only the network listener task
- [ ] 3.2.4 Run `cargo test && cargo clippy -- -D warnings`

### 3.3 `ProviderParser` trait in parser module
- [ ] 3.3.1 Define `pub(crate) trait ProviderParser` in `src/parser/mod.rs` with `parse_request`, `extract_delta` methods
- [ ] 3.3.2 Implement the trait for each of `anthropic`, `openai`, `google` parser structs (or unit structs)
- [ ] 3.3.3 Replace the first `match provider { … }` block in `parse_request()` with trait dispatch
- [ ] 3.3.4 Replace the second `match provider { … }` block in `extract_sse_delta()` with trait dispatch
- [ ] 3.3.5 Run `cargo test && cargo clippy -- -D warnings` — parser tests still pass

**Dependencies**: Phase 3 tasks are independent of each other (3.1, 3.2, 3.3 can be done in any order or in parallel)
**Note**: coordinate with `add-test-coverage` branch before merging 3.2 (both touch `main.rs`)

---

## Phase 4 — Polish (`network.rs`, `ca/`, `config.rs`)

**Goal**: replace magic literals with named constants, remove dead code, improve startup safety.

### 4.1 Named constants in `network.rs`
- [ ] 4.1.1 Add at top of `network.rs`:
  ```rust
  const TLS_RECORD_TYPE_HANDSHAKE: u8 = 0x16;
  const CLIENT_HELLO_TYPE: u8 = 0x01;
  const DNS_COMPRESSION_FLAG: u8 = 0xC0;
  const SNI_EXT_TYPE: u16 = 0x0000;
  const DNS_A_RECORD_TYPE: u16 = 1;
  ```
- [ ] 4.1.2 Replace all inline hex/numeric literals in `peek_sni()`, `build_dns_a_query()`, `parse_first_a_record()` with the new constants
- [ ] 4.1.3 Run `cargo clippy -- -D warnings`

### 4.2 CA module cleanup
- [ ] 4.2.1 Add `pub(crate) const CA_ORG_NAME: &str = "Privacyclaw Privacy Proxy";` and `pub(crate) const CA_COMMON_NAME: &str = "Privacyclaw Root CA";` in `ca/mod.rs`
- [ ] 4.2.2 Replace the hardcoded string literals in `generate_ca()` (`ca/mod.rs`) with the new constants
- [ ] 4.2.3 Replace the hardcoded string literals in `build_certified_key()` (`ca/cert_gen.rs`) with `crate::ca::CA_ORG_NAME` / `crate::ca::CA_COMMON_NAME`
- [ ] 4.2.4 Remove the `cert_pem` field from `CaBundle` struct (it is never read after construction); update `generate_ca()` and any `CaBundle { … }` construction sites
- [ ] 4.2.5 Rename `SingleKeyResolver` → `StaticKeyResolver` in `cert_gen.rs`; update all references
- [ ] 4.2.6 Run `cargo build && cargo test && cargo clippy -- -D warnings`

### 4.3 Config validation
- [ ] 4.3.1 Add `pub fn validate(&self) -> Result<()>` on `Config` in `config.rs` checking:
  - `proxy.listen` and `proxy.dashboard` parse as `SocketAddr` (validates host + port)
  - `network_proxy.listen` parses as `SocketAddr`
  - `pii.ner.timeout_ms > 0` and `pii.slm.timeout_ms > 0`
  - `storage.logs_dir` is non-empty
- [ ] 4.3.2 Call `cfg.validate()?` inside `Config::load()` after successful TOML parse (before returning)
- [ ] 4.3.3 Add unit tests for `validate()`: valid config passes, invalid port fails, zero timeout fails, empty logs_dir fails
- [ ] 4.3.4 Run `cargo test && cargo clippy -- -D warnings`

**Dependencies**: 4.1, 4.2, 4.3 are fully independent — do in any order or parallel

---

## Final validation (all phases)

- [ ] F.1 `cargo build` — zero errors, zero warnings
- [ ] F.2 `cargo test` — all tests pass (≥175), 0 failed
- [ ] F.3 `cargo clippy -- -D warnings` — zero suppressions added by this change
- [ ] F.4 Confirm `#[allow(clippy::too_many_arguments)]` does not appear anywhere in `intercept.rs`
- [ ] F.5 Confirm `conv_file_path()` no longer calls `read_dir()` on the hot path (grep check)
- [ ] F.6 Confirm `passthrough` logic appears in exactly one file (`proxy/passthrough.rs`)
