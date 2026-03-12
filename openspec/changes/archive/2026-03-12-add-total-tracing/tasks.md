## Phase 1: Critical — Payload-Bearing Log Calls (unblocked)

- [x] 1.1 Fix subscriber initialization order in `src/main.rs`: move `tracing_subscriber` setup to the very first lines of `main()`, before `Config::load`, so no log records are silently dropped during startup
  - _Note: subscriber init stays after `Config::load` by design — calling `init()` twice panics. The one dropped event (config.rs:204, no-config-file debug) is acceptable. `WorkerGuard` kept alive until `main()` returns._
- [x] 1.2 Fix format-string violations in `src/pii/vault.rs`: convert all `tracing::warn!("text {}", var)` calls to structured form `tracing::warn!(field = %var, "text")`
- [x] 1.3 Fix format-string violations in `src/proxy/intercept.rs`: convert all `tracing::debug!("text {}", var)` and `tracing::error!("text {}", var)` calls to structured key=value fields
- [x] 1.4 Add `tracing::info!` and `tracing::debug!` to `src/pii/synth.rs`:
  - `get_or_create`: TRACE on entry, cache hit, cache miss; DEBUG with `original`/`synthetic` on first generation
  - _Note: `original`/`synthetic` demoted from INFO → DEBUG (privacy fix — raw PII must not appear in production logs at default INFO level)_
  - `generate`: TRACE on dispatcher entry
- [x] 1.5 Add `tracing::debug!` and `tracing::trace!` to `src/pii/buffer.rs`:
  - `process_delta`: TRACE at entry, on prefix refresh, vault-empty path, replace_synthetics call, holdback decision; DEBUG at each exit path with `flushed_len`/`holdback_len`
  - `flush_remaining`: DEBUG on call
- [x] 1.6 Add `tracing::debug!` and `tracing::info!` to `src/pii/tier1.rs`:
  - `find_all`: TRACE per regex match and per validator call; DEBUG at `detect` entry and exit with `span_count`
  - _Note: per-call INFO removed (contrarian: INFO on every scan pollutes production logs); WARN on clean scans removed (expected case, not anomalous)_
- [x] 1.7 Add `tracing::debug!`, `tracing::info!`, `tracing::warn!` to `src/pii/vault.rs`:
  - `add_mapping`: TRACE at entry, DEBUG after completion with `mapping_count`/`max_key_len`
  - `replace_synthetics`: INFO when `any == true`
  - Format-string WARN fixed to structured fields
- [x] 1.8 Add `tracing::debug!` to `src/pii/mod.rs` `detect_spans`: DEBUG after tier1 with `t1_span_count`, `text_len`

## Phase 2: Infrastructure — JSON Formatter and File Output (needs Phase 1 complete)

- [x] 2.1 Add `tracing-appender = "0.2"` to `claudovka/Cargo.toml`; enable the `json` and `registry` features on the existing `tracing-subscriber` dependency
- [x] 2.2 Extend `LoggingConfig` in `src/config.rs`:
  - `format: String` (default `"text"` — backward-compatible; set `"json"` to opt in)
  - `file: Option<String>` (default `None` — file logging off by default to prevent disk exhaustion at TRACE level)
  - `rotation: String` (default `"daily"`)
- [x] 2.3 Rewrite the subscriber initialization block in `src/main.rs` to use a layered subscriber:
  - `Registry + EnvFilter + fmt::Layer(stderr, non_blocking) + optional fmt::Layer(file, non_blocking)`
  - `make_file_appender()` helper extracted to eliminate duplication
  - `Vec<WorkerGuard>` held until `main()` / `run_tray_mode()` returns
- [x] 2.4 Add `--log-file <PATH>` global flag to the CLI in `src/main.rs`: overrides `cfg.logging.file` before subscriber init; empty string disables file output even if set in config
- [x] 2.5 Update `config.example.toml` to document `format`, `file`, `rotation` with inline comments
- [x] 2.6 Verified: `cargo clippy -- -D warnings` clean; `cargo test` — all tests pass (370+ tests)

## Phase 3: TRACE Saturation — Full Decision-Tree Visibility (needs Phase 2 complete)

- [x] 3.1 Add `tracing::trace!` to every conditional branch in `src/pii/tier1.rs`: per-match TRACE (entity_type, span_start, span_end) and per-validator TRACE (entity_type, valid) inside the `find_all` loop cover all 14+ pattern paths
- [x] 3.2 Add `tracing::trace!` to `src/pii/synth.rs` generation internals: TRACE on generator dispatch in `generate()`; per-char substitution skipped (too granular — would produce thousands of records per request; architect explicitly excluded this)
- [x] 3.3 Add `tracing::trace!` to `src/pii/buffer.rs` holdback path: TRACE on holdback decision with `safe_len`, `has_trigger`, `replaced_len`; per-byte loop body skipped (architect: per-byte is too noisy; holdback-decision TRACE is the right granularity)
- [x] 3.4 Add `tracing::trace!` to `src/proxy/intercept.rs` SSE delta path: TRACE with `conv_id`, `delta_text_len`, `after_replace_len` on every SSE event processed through the replacement buffer
- [x] 3.5 Add `tracing::trace!` to vault serialization/reload path: TRACE per mapping restored from disk (in `storage/mod.rs::load_vault`) with `conv_id`, `original_len`, `synthetic_len`, `entity_type`, `tier`
- [x] 3.6 `cargo test` passes (all suites); JSON output verified compilable via `tracing-subscriber` json feature; `RUST_LOG=trace cargo run -- --help` emits structured JSON to stderr
