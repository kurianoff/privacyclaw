# Change: Add PII Detection and Bidirectional Replacement (Phase 2)

## Why

Phase 1 proves the MITM intercept architecture works: all LLM API traffic is captured, parsed, and displayed. However, the proxy is currently read-only — real PII (names, emails, API keys, medical data) flows to commercial LLM providers in plaintext. Phase 2 adds the core privacy feature: detect PII in outbound requests, replace with synthetic equivalents, and silently reverse the replacements in streaming responses — so the end user's Claude Code / Cursor session works identically while the LLM never sees real PII.

## What Changes

- **BREAKING — outbound request path**: `intercept.rs` currently forwards request bytes to upstream immediately. Phase 2 buffers the entire request body, runs PII detection, rewrites the JSON, and forwards the modified request. This adds latency to the request path (Tier 1: <2ms; Tier 2: 20-50ms).
- **BREAKING — Content-Length**: When PII is replaced, the modified body length may differ. The proxy must update the `Content-Length` header before forwarding.
- **New — inbound replacement buffer**: SSE response chunks are forwarded through a prefix-aware `ReplacementBuffer` that recognises synthetic tokens in mid-stream and swaps them back to originals.
- **New — PII Vault**: Per-conversation bidirectional mapping store with Aho-Corasick automaton for reverse replacement.
- **New — Tier 1 regex**: Fast (<2ms) in-process detection of structured PII (email, phone, SSN, credit card, IP, API keys, etc.) using the `regex` + `fancy-regex` crates with patterns ported from Microsoft Presidio.
- **New — Tier 2 GLiNER NER** (optional): ONNX model inference via `ort` crate for unstructured PII (names, addresses, orgs) not caught by regex. Off by default; loaded on demand.
- **New — Tier 3 SLM sidecar** (optional): HTTP client to llama-server for context-aware disambiguation of ambiguous spans. Off by default.
- **New — Synthetic data generation**: `fake` crate used to generate culturally-appropriate replacements keyed by PII type.
- **New — CLI commands**: `test-pii`, `models install`, `models list`, `benchmark`.
- **New — Dashboard PII panel**: Visual diff of original vs sanitised text; vault mapping table per conversation.
- **Storage schema extension**: Vault mappings persisted as an additional NDJSON entry per conversation file.

## Impact

- **Affected specs**: `mitm-proxy`, `cli`, `storage`, `dashboard`
- **New specs**: `pii-vault`, `pii-pipeline`
- **Affected code**:
  - `src/proxy/intercept.rs` — outbound buffering, vault injection, inbound replacement buffer
  - `src/config.rs` — new `[pii]` section
  - `src/main.rs` — vault registry init, new CLI subcommands
  - `src/storage/mod.rs` — vault persistence
  - `src/dashboard/mod.rs` — PII panel WebSocket events + REST endpoints
  - **New**: `src/pii/` module tree (vault, tier1, tier2, tier3, synth, buffer, locale)
  - **New**: `src/models/` module (download, registry)
- **New dependencies**: `aho-corasick`, `regex`, `fancy-regex`, `ort` (optional feature), `tokenizers` (optional), `fake`, `rand`, `reqwest` (for model download + SLM sidecar)
- **Cross-cutting concern**: `PiiVaultRegistry` is a new shared singleton passed through the proxy call stack alongside `Store` and `ws_tx`.
