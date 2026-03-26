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

