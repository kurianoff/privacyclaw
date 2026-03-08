# Design: Structured Three-Level Async Logging

## Context

Claudovka uses `tracing` (already a dependency) throughout the async proxy stack. The change adds instrumentation to all source files without changing runtime behavior. The design must address: async safety, data volume at DEBUG, sensitive data exposure, and consistent level semantics.

## Goals / Non-Goals

- **Goals**: Full code-path coverage at DEBUG; atomic-operation coverage at INFO; lifecycle-event coverage at WARN; all non-blocking; structured fields (key=value)
- **Non-Goals**: Log aggregation/shipping; span/trace propagation (no OpenTelemetry spans this phase); per-connection log correlation IDs; log sampling/rate-limiting

## Decisions

### Decision: Use existing `tracing` crate, no new dependencies

`tracing` macros (`trace!`, `debug!`, `info!`, `warn!`, `error!`) are zero-overhead when the subscriber's max level is above the macro's level. No allocation, no formatting, no side effects. The subscriber is already configured in `main.rs` via `tracing_subscriber::fmt`. No new crates needed.

Alternatives considered:
- `log` crate: rejected — less structured, no async span support
- `slog`: rejected — already committed to `tracing`

### Decision: Truncate raw bytes at 256 bytes for DEBUG chunk logs

At DEBUG level, every read chunk in `intercept.rs` and `network.rs` is logged. With 65 KB read buffers, logging full bytes would produce ~1 MB of log output per SSE response. Truncate to 256 bytes and include `chunk_total_bytes` field.

Format: `chunk_hex = "48 54 54 50 2f 31..."` (space-separated lowercase hex, up to 256 bytes).

Alternatives considered:
- Base64: less readable for manual debugging
- Full bytes: too verbose; 2000-event SSE = ~130 MB of hex logs

### Decision: Redact `Authorization` header value at DEBUG

The proxy sees Bearer tokens (`x-api-key`, `Authorization`) in plaintext. DEBUG logs include HTTP headers; replace the value of any header whose lowercased name is `authorization` or `x-api-key` with `"[REDACTED]"`.

This is the minimum viable approach — full PII protection is tracked in the `add-pii-protection` change.

### Decision: Promote listener-bound events from INFO to WARN

The user spec assigns WARN to "operational block" events: starting/stopping listening on a port. Currently these are logged at INFO in `proxy/mod.rs`, `proxy/network.rs`, and `dashboard/mod.rs`. All three are changed to WARN to match the spec. Existing callers of `tracing::info!` for these events are updated.

### Decision: Add `fmt_chunk_hex` helper to `util.rs`

A tiny helper `fmt_chunk_hex(data: &[u8], max: usize) -> String` formats bytes as hex and adds a `...(N total)` suffix when truncated. Keeps DEBUG instrumentation sites clean.

## Risks / Trade-offs

- **Risk**: Very high DEBUG log volume during SSE streaming (one log per chunk, ~65 KB chunks, 2000 events). **Mitigation**: Users enable DEBUG only during active debugging; INFO is the default in config.
- **Risk**: Forgetting to redact a new sensitive header added in future. **Mitigation**: `fmt_chunk_hex` is used for body bytes only; headers are formatted via a dedicated `fmt_headers` helper that redacts known sensitive keys.
- **Risk**: `tracing::debug!` inside tight loops (u2c/c2u) marginally increases CPU when DEBUG is enabled. **Mitigation**: Acceptable for a debugging build; macros are no-ops at INFO and above.

## Migration Plan

No migration needed — pure additive change. All existing log callsites are preserved; new callsites are added around/within them.

## Open Questions

- Should we add `tracing::Span` per-connection for correlation? Deferred to a follow-up change.
- Should DEBUG body logs include full SSE data payloads (not just chunk bytes)? Currently scoped to raw TCP chunk bytes; SSE event `data:` field values are also included in DEBUG inside `process_response_chunk`.
