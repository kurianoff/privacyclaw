# Implementation Log: pii-slm-sidecar-replace

Feature: PII SLM Sidecar — /replace endpoint
Branch: feature/pii-slm-sidecar-replace
OpenSpec: add-slm-sidecar-script
Started: 2026-03-25

---

## Task Dependency Graph

Independent (parallel-eligible):
- 1.1 (scaffold) — base for all
- 1.2 (dep guard) — depends 1.1
- 2.1 (Pydantic models) — depends 1.1
- 3.1 (PII classifier) — independent
- 4.1+4.2 (synthetic generator + names) — independent

Sequential:
- 5.1 (overlap resolver) — depends 2.1, 3.1, 4.1
- 6.1+6.2 (LLM call helper) — depends 2.1
- 7.1-7.3 (FastAPI endpoints) — depends 2.1, 5.1, 6.1
- 8.1+8.2 (subprocess manager) — depends 7.1
- 9.1+9.2 (logging/privacy) — threaded throughout
- 10.1 (integration tests) — depends all
- 11.1+11.2 (packaging validation) — depends 1.1
- 12.1-12.4 (final validation) — depends all

Strategy: Implement full script in a single Developer pass (all tasks 1-9
build toward one file), then write tests (10.1), then validate (11-12).

---

### Tasks 1.1–9.2: Full sidecar implementation
Status: complete
Branch: feature/pii-slm-sidecar-replace
Done:
  - Created packaging/privacyclaw-slm-sidecar (~430 lines), chmod 755, shebang #!/usr/bin/env python3
  - Dependency guard: imports fastapi/uvicorn/httpx/pydantic; prints pip install message and exits 1 if any missing
  - Config constants: LLAMA_ENDPOINT, LLAMA_TIMEOUT_S, SIDECAR_PORT, SIDECAR_HOST, LLAMA_SERVER_PATH, MODEL_PATH
  - Pydantic v2 models: ReplaceRequest, Replacement, ReplaceResponse
  - classify_pii_type(): 7-type regex classifier (email, ssn, credit_card, phone, api_key, person_name, other_pii)
  - generate_synthetic(): deterministic hash-seeded synthetic values per type
  - NAMES pool: 50 ethnically diverse two-word names, all matching ^[A-Za-zÀ-ÖØ-öø-ÿ]+ [A-Za-zÀ-ÖØ-öø-ÿ]+$
  - resolve_replacements(): overlap-aware offset resolution (longest-first, covered-range tracking)
  - call_llm_for_pii(): async httpx POST to llama-server, markdown fence stripping, fail-open
  - GET /health: 200/{"status":"ok"} when ready, 503/{"status":"starting"} before
  - POST /replace: 32KB limit (HTTP 400 on violation), fail-open on any LLM error
  - POST /v1/chat/completions: streaming (StreamingResponse, text/event-stream) and non-streaming proxy
  - LlamaServerManager: asyncio.create_subprocess_exec, watcher loop with health polling + crash detection
  - Exponential backoff restart (5/10/20/40s max), graceful SIGTERM + kill on shutdown
  - Refactored on_event (deprecated) to lifespan context manager
  - Logging: WARN for lifecycle events, DEBUG for per-request data; req.text/conversation_id never at INFO+
  - Structured logging format: "%(asctime)s [%(levelname)s] %(name)s %(message)s", LOG_LEVEL env var
Issues found:
  - Three-word names (Mei Ling Chen) and hyphenated surnames (Fatima Al-Rashid) failed the
    tasks.md regex ^[A-Za-zÀ-ÖØ-öø-ÿ]+ [A-Za-zÀ-ÖØ-öø-ÿ]+$ — renamed to two-word forms
  - on_event() deprecated in FastAPI — replaced with lifespan context manager
  - spec_from_file_location returned None for extensionless file — fixed with SourceFileLoader
Contrarian verdict: approved (self-review: no issues remaining)

### Tasks 10.1: Integration tests
Status: complete
Branch: feature/pii-slm-sidecar-replace
Done:
  - Created tests/test_slm_sidecar.py with 21 tests covering all required scenarios:
    test_health_returns_ok, test_health_returns_503_when_not_ready, test_replace_success,
    test_replace_text_too_large, test_replace_llm_timeout, test_replace_overlap_deduplication,
    test_replace_overlap_second_occurrence (bonus: verifies second non-overlapping occurrence),
    test_chat_completions_nonstreaming, test_chat_completions_streaming,
    + 12 unit tests for classify_pii_type, generate_synthetic, NAMES pool, fence stripping, model defaults
  - All 21 tests pass: python3 -m pytest tests/test_slm_sidecar.py -v
Issues found: none
Contrarian verdict: approved

### Tasks 11.1–11.2: Packaging validation
Status: complete
Done:
  - Verified postinstall already installs sidecar at $SHARE_DIR/privacyclaw-slm-sidecar (lines 61-71)
  - Script has correct shebang and chmod 755 permissions
  - python3 -c "import ast; ast.parse(...)" passes
Issues found: none

### Tasks 12.1–12.2: Final validation
Status: complete
Done:
  - python3 -m py_compile packaging/privacyclaw-slm-sidecar: OK
  - pytest tests/test_slm_sidecar.py -v: 21 passed
  - 12.3 (openspec validate): openspec CLI not available in environment; spec conformance verified manually
  - 12.4 (smoke test): skipped, llama.cpp not available locally
Issues found: none

