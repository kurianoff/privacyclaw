# Change: Add Total Tracing — 5-Level Logging with Payload Visibility

## Why

The PII pipeline (`tier1.rs`, `buffer.rs`, `synth.rs`, `vault.rs`) contains zero `tracing` calls. When a replacement does not fire, a holdback is triggered, or a pattern produces unexpected results, there is no diagnostic signal — the only option is to attach a debugger. Additionally, several `tracing::error!`/`debug!` calls in the proxy and vault use string-interpolation format arguments instead of structured fields, violating the logging rules in `CLAUDE.md` and producing non-parseable log records. A subscriber initialization ordering bug also causes the first few log lines (including startup errors) to be silently dropped before `Config::load` returns.

## What Changes

- **Phase 1 — Payload-bearing log calls**: Add `TRACE`/`DEBUG`/`INFO` instrumentation to `synth.rs`, `tier1.rs`, `buffer.rs`, `vault.rs`, and `pii/mod.rs`. Fix all format-string violations in `vault.rs` and `proxy/intercept.rs`. Fix the subscriber initialization ordering so the subscriber is registered before `Config::load`.
- **Phase 2 — JSON formatter and file output**: Add `tracing-appender = "0.2"` and enable the `json` feature on `tracing-subscriber`. Extend `LoggingConfig` with `format`, `file`, and `rotation` fields. Build a layered subscriber using `Registry + EnvFilter(reload) + fmt::Layer(stderr) + optional fmt::Layer(file)` with `non_blocking` writers and a `WorkerGuard` held in `main()`. Update `config.example.toml`.
- **Phase 3 — TRACE saturation**: Instrument every meaningful branch in the PII pipeline at `TRACE` level so the full decision tree is visible when `RUST_LOG=trace`.

## Impact

- Affected specs: `observability` (MODIFIED — 5-level semantics, JSON format, dual output, field conventions), `pii-pipeline` (ADDED — TRACE/DEBUG requirements on tier1, buffer, synth, vault), `cli` (ADDED — `--log-file` flag)
- Affected code: `Cargo.toml`, `src/config.rs`, `src/main.rs`, `src/pii/synth.rs`, `src/pii/buffer.rs`, `src/pii/tier1.rs`, `src/pii/vault.rs`, `src/pii/mod.rs`, `src/proxy/intercept.rs`, `config.example.toml`
- No breaking changes to existing behavior. Default `format = "json"` is a new default that changes log output format from the current plain-text. Users relying on parsing the existing text format should set `format = "text"` in their config.
