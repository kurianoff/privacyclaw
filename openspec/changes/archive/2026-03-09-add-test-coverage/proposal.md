# Change: Add comprehensive test coverage

## Why

The proxy core (intercept pipeline, storage, parsers, SSE handling) has no automated correctness tests. A critical bug — TLS flush missing after large requests — went undetected and caused 5-minute stalls in production. Without a test suite, regressions will recur and behavioral contracts have no machine-checkable form.

## What Changes

- Add unit tests for SSE parser edge cases (extends existing 6 tests)
- Add unit tests for all LLM request/response parsers (Anthropic, OpenAI, Google)
- Add unit tests for storage correctness, fingerprinting, and concurrency
- Add integration tests for the proxy pipeline: roundtrip fidelity, flush regression, multi-turn keep-alive, concurrent sessions, upstream failure modes
- Add observability correctness tests: credential redaction, hex truncation
- Add network helper tests: SNI extraction, DNS query packet format

## Impact

- Affected specs: mitm-proxy, storage, llm-parser, observability, sse-parser
- Affected code: `src/proxy/intercept.rs`, `src/storage/mod.rs`, `src/parser/`, `src/parser/sse.rs`, `src/util.rs`, `src/proxy/network.rs`
- No behavior changes — tests verify existing contracts
