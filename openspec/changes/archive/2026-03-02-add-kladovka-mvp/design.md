# Design: Kladovka MVP

## Context

Kladovka is a local developer tool that acts as an HTTPS MITM proxy to observe traffic between AI coding agents and LLM APIs. Phase 1 is observation-only; no modifications to traffic.

Stakeholders: individual developers and security-conscious teams wanting visibility into LLM data flows.

## Goals / Non-Goals

Goals:
- Intercept and decode HTTPS traffic to configured LLM API endpoints
- Display full conversations (including streaming tokens) in a real-time dashboard
- Persist all traffic to local SQLite for review
- Never add latency or break non-LLM traffic
- Single binary, no runtime dependencies

Non-Goals:
- PII redaction or modification (Phase 2+)
- Multi-user or networked deployment
- MCP / tool call traffic (Phase 2+)

## Decisions

**Decision: rustls over native-tls**
- Chosen for cross-platform consistency, no OpenSSL dependency, easier static linking
- Alternative: native-tls — rejected due to platform-specific behavior and dynamic linking

**Decision: tee approach for zero-latency passthrough**
- Raw bytes forwarded immediately via `tokio::io::copy`; a cloned byte stream is sent to parser
- Alternative: buffer-then-forward — rejected because it adds latency on the critical path

**Decision: rcgen for dynamic leaf cert generation**
- Pure Rust, no external tools, supports P-256 ECDSA
- Alternative: call openssl CLI — rejected for portability and subprocess overhead

**Decision: tokio-tungstenite for WebSocket**
- Integrates naturally with tokio; well-maintained
- Alternative: axum with built-in WS — rejected to minimize framework dependencies

**Decision: embedded static assets via rust-embed**
- Single binary with no external file dependencies
- Alternative: serve from disk at runtime — rejected for deployment simplicity

**Decision: SQLite via rusqlite bundled**
- Zero external DB dependency, sufficient for local single-user workload
- Alternative: PostgreSQL — far too heavy for a local dev tool

## Data Flow

```
Client → TCP → Proxy
  CONNECT host:port → Parse host
  if host in allowlist:
    → Generate leaf cert (cached)
    → TLS handshake with client (present leaf cert)
    → TLS connect to upstream
    → Bidirectional decrypted forwarding
      → Tee: copy bytes to client AND to parser channel
      → Parser: extract model, messages, SSE deltas
      → Storage: save to SQLite
      → Dashboard: broadcast via WebSocket
  else:
    → Raw TCP tunnel (passthrough)
```

## Risks / Trade-offs

- Dynamic cert generation adds ~1ms per new domain (mitigated by in-memory cache)
- SSE accumulation uses memory; capped at 10MB per response
- Trust store installation requires elevated permissions on some platforms

## Migration Plan

Greenfield. No existing code to migrate.

## Open Questions

- None for Phase 1
