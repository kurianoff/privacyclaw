# Tasks: add-kladovka-mvp

## 1. Project Scaffold

- [ ] 1.1 Create `Cargo.toml` with all declared crate dependencies
- [ ] 1.2 Create `config.example.toml`
- [ ] 1.3 Create `src/main.rs` with CLI skeleton (clap subcommands: init, start, ca-path, reset-ca, export)
- [ ] 1.4 Create `src/config.rs` with TOML loading and sensible defaults

## 2. CA Management

- [ ] 2.1 Create `src/ca/mod.rs`: generate ECDSA P-256 CA key + self-signed cert via rcgen
- [ ] 2.2 Platform-specific storage paths via `dirs` crate
- [ ] 2.3 Create `src/ca/cert_gen.rs`: dynamic leaf cert generation cached by domain
- [ ] 2.4 Implement trust store installation for macOS / Linux / Windows behind `--install-ca` flag
- [ ] 2.5 Implement `reset-ca` to delete and regenerate

## 3. MITM Proxy Core

- [ ] 3.1 Create `src/proxy/mod.rs`: TCP listener accept loop
- [ ] 3.2 Create `src/proxy/connect.rs`: parse HTTP CONNECT, route to MITM or passthrough
- [ ] 3.3 Create `src/proxy/passthrough.rs`: raw bidirectional TCP tunnel
- [ ] 3.4 Create `src/proxy/intercept.rs`: TLS termination, tee copy, channel to parser

## 4. LLM Parser

- [ ] 4.1 Create `src/parser/mod.rs`: provider detection by domain, dispatch to per-provider parsers
- [ ] 4.2 Create `src/parser/sse.rs`: generic SSE stream parser (partial chunks, multi-line data, [DONE])
- [ ] 4.3 Create `src/parser/anthropic.rs`: Anthropic Messages API request + SSE response parsing
- [ ] 4.4 Create `src/parser/openai.rs`: OpenAI Chat Completions request + SSE response parsing
- [ ] 4.5 Create `src/parser/google.rs`: Google Gemini API request + SSE response parsing

## 5. Storage

- [ ] 5.1 Create `src/storage/schema.sql`: conversations + messages tables + index
- [ ] 5.2 Create `src/storage/mod.rs`: SQLite init, insert conversation, insert message, query, prune

## 6. Web Dashboard

- [ ] 6.1 Create `src/dashboard/assets/index.html`: single-page chat-style dashboard
- [ ] 6.2 Create `src/dashboard/assets/style.css`: styling for conversation list, chat bubbles, streaming cursor
- [ ] 6.3 Create `src/dashboard/assets/app.js`: WebSocket client, conversation renderer, live token appending
- [ ] 6.4 Create `src/dashboard/mod.rs`: HTTP server (REST + WebSocket), rust-embed assets, WS broadcast

## 7. Utilities

- [ ] 7.1 Create `src/util.rs`: UUID generation, ISO 8601 timestamps, gzip compression helpers

## 8. Tests

- [ ] 8.1 Unit tests for SSE parser (edge cases: partial chunks, empty events, multi-line data, [DONE])
- [ ] 8.2 Integration test: CONNECT tunnel establishment
- [ ] 8.3 Create test fixtures directory with sample SSE byte streams
