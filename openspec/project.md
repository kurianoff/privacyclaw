# Project Context

## Purpose

**Privacyclaw** ("storage room" in Russian) is a local MITM privacy proxy that intercepts HTTPS traffic between AI coding agents (Claude Code, Cursor, Codex, etc.) and commercial LLM APIs. It decrypts, parses, and displays all request/response traffic — including streaming SSE — in a real-time web dashboard.

Phase 1 (MVP): observation mode only — no PII redaction, no modification. Goal: prove the core architecture.

## Tech Stack

- **Language**: Rust (stable, 2021 edition)

- **Async runtime**: tokio (full features)

- **HTTP**: hyper v1 + hyper-util + http-body-util

- **TLS**: rustls v0.23 + tokio-rustls v0.26

- **Cert generation**: rcgen v0.13

- **Database**: SQLite via rusqlite (bundled feature)

- **Serialization**: serde + serde_json

- **Config**: toml

- **CLI**: clap v4 (derive)

- **Logging**: tracing + tracing-subscriber

- **WebSocket**: tokio-tungstenite v0.24

- **Static assets**: rust-embed v8

- **Utilities**: uuid v1, chrono v0.4, flate2 v1, dirs v6, webpki-roots v0.26

## Project Conventions

### Code Style

- Rust 2021 edition
- `rustfmt` defaults
- `clippy` clean (no warnings)
- Error propagation via `anyhow::Result` for application code; typed errors in library-facing modules
- All public APIs have doc comments

### Architecture Patterns

- Modular: `ca/`, `proxy/`, `parser/`, `storage/`, `dashboard/` as separate modules
- Zero-copy passthrough: parse on a tee'd copy, never block the critical path
- Phase 1 is read-only: never modify request/response bytes
- Single static binary with embedded web assets

### Testing Strategy

- Unit tests for SSE parser edge cases
- Integration tests with recorded HTTP/SSE fixtures (no live API calls)
- Passthrough safety tests

### Git Workflow

- Feature branches, squash merge
- Conventional commits: `feat:`, `fix:`, `docs:`, `refactor:`, `test:`

## Domain Context

The proxy uses HTTP CONNECT tunneling. Clients send `CONNECT host:port HTTP/1.1`, the proxy either:

- **MITM** (allowlisted LLM domains): generates leaf cert signed by local CA, terminates TLS on both sides, reads/logs traffic
- **Passthrough** (all other domains): raw TCP tunnel, no TLS termination, original server cert presented

## Important Constraints

- **Zero latency on passthrough**: bytes forwarded as they arrive; parsing is off-path
- **No panics**: graceful degradation on malformed input
- **Memory bounded**: SSE accumulation buffers capped at 10MB per response
- **Cross-platform**: macOS (ARM64 + x86_64), Linux (x86_64 + ARM64), Windows (x86_64)
- **Phase 1 read-only**: proxy MUST NOT modify any request or response bytes

## External Dependencies

- `api.anthropic.com` — Anthropic Messages API (SSE streaming)
- `api.openai.com` — OpenAI Chat Completions API (SSE streaming)
- `generativelanguage.googleapis.com` — Google Gemini API
- `api.mistral.ai` — Mistral AI
- `api.groq.com` — Groq
