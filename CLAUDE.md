<!-- OPENSPEC:START -->
# OpenSpec Instructions

These instructions are for AI assistants working in this project.

Always open `@/openspec/AGENTS.md` when the request:
- Mentions planning or proposals (words like proposal, spec, change, plan)
- Introduces new capabilities, breaking changes, architecture shifts, or big performance/security work
- Sounds ambiguous and you need the authoritative spec before coding

Use `@/openspec/AGENTS.md` to learn:
- How to create and apply change proposals
- Spec format and conventions
- Project structure and guidelines

Keep this managed block so 'openspec update' can refresh the instructions.

<!-- OPENSPEC:END -->

# Kladovka

Privacy proxy for AI agent traffic. Rust project.

## Conventions
- Rust 2021 edition, async with tokio
- Error handling: anyhow for app code, thiserror for library crates
- Logging: tracing crate, structured fields
- Tests: #[tokio::test] for async, fixtures in tests/fixtures/
- Formatting: cargo fmt, cargo clippy -- -D warnings
- Commit style: conventional commits (feat:, fix:, docs:)

## Logging Rules

Three-tier structured logging with `tracing` macros. Default level: INFO.

- **WARN** — lifecycle events: proxy/dashboard/CA bound, mode started/stopped, connected/disconnected, cert generated, CA installed, log rotation. One line per operational block.
- **INFO** — atomic operations: connection accepted, chunk read/forwarded (with byte count), request body received, response complete, store insert ok, SSE `[DONE]`, WS client connected/event sent.
- **DEBUG** — every branch + raw data: loop state, chunk hex (truncated to 256 bytes), HTTP headers (auth redacted), SSE event data, cache hit/miss, DNS steps, per-parser delta in/out.

Rules:

- Use structured fields (`key = %val` or `key = ?val`), not format strings in the message.
- Redact `Authorization` and `X-Api-Key` header values in all log output (`fmt_headers` helper in `util.rs`).
- Truncate raw byte dumps to 256 bytes (`fmt_chunk_hex` helper in `util.rs`).
- Never log inside a held `Mutex` lock beyond the minimal critical section.
- Override at runtime: `RUST_LOG=claudovka=debug` or via `logging.level` in config TOML.

## Architecture
See docs/TASK-phase1.md for current implementation plan.

## Commands
- `cargo build` — build (debug mode; no `--release` needed during development)
- `cargo test` — run tests
- `cargo run -- init` — generate CA
- `cargo run -- start` — start proxy
- `cargo run -- network-start` — start network proxy

Note: always use debug builds (`cargo build`, `cargo run`) during development. Release builds are only for distribution.
