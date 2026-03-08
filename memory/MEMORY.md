# Kladovka Project Memory

## Project Overview
"Kladovka" (storage room in Russian) — local MITM privacy proxy for LLM API traffic inspection.
Located at: `kladovka/` (Rust binary)

## Key Architecture
- HTTP CONNECT proxy on `127.0.0.1:8080`
- Web dashboard on `127.0.0.1:8443`
- SQLite storage at `~/.config/kladovka/data.db`
- CA cert at `~/Library/Application Support/kladovka/ca/`
- Zero-latency: bytes forwarded immediately, parsing happens on a tee'd copy

## Rust Toolchain Constraint
- Rust 1.85.0 on this machine
- `time` crate must be pinned: `cargo update time --precise 0.3.36 && cargo update time-core --precise 0.1.2`
- `rcgen 0.13` does NOT have `from_ca_cert_pem` — reconstruct CA cert from key + params

## Modules
- `src/ca/` — CA generation (rcgen), leaf cert cache (CertCache)
- `src/proxy/` — TCP listener, CONNECT handler, MITM intercept, passthrough
- `src/parser/` — SSE parser, Anthropic/OpenAI/Google protocol parsers
- `src/storage/` — SQLite via rusqlite (bundled)
- `src/dashboard/` — HTTP + WebSocket server with embedded assets (rust-embed)
- `src/config.rs` — TOML config with defaults
- `src/util.rs` — UUID, timestamps, gzip

## OpenSpec Status
- Change `add-kladovka-mvp` archived on 2026-03-02
- 6 capabilities created: ca-management, cli, dashboard, llm-parser, mitm-proxy, storage

## Compile & Test
```bash
cd kladovka && cargo build && cargo test
```
All 6 SSE parser unit tests pass.
