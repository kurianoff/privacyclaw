# Change: Add PII SLM Sidecar Script with /replace Endpoint

## Why

The T3-first PII pipeline (change `update-pii-t3-first-pipeline`) calls
`POST /replace` on the SLM sidecar endpoint. No sidecar script exists yet.
The Rust proxy already ships a bundled `llama-server` binary (Option B packaging
strategy); the sidecar must manage that binary as a subprocess so users get T3
PII detection without any manual setup.

## What Changes

- **New file**: `packaging/privacyclaw-slm-sidecar` — single Python 3 executable
  that exposes the three endpoints the Rust proxy requires. It starts and manages
  the bundled `llama-server` as a child subprocess (start on boot, health-poll
  every 5 s, restart on crash with exponential backoff).
- **New capability**: `POST /replace` — calls the local SLM to detect PII strings,
  resolves byte offsets (overlap-aware, longest-first), generates deterministic
  hash-seeded synthetic display values, returns structured `ReplaceResponse`.
- **New capability**: `POST /v1/chat/completions` — streaming-aware proxy pass-
  through to llama-server; required for `SlmSidecar::disambiguate()` in Rust.
- **New capability**: `GET /health` — readiness probe; returns `{"status": "ok"}`
  only after llama-server is itself healthy.
- **Request safety**: text larger than 32 KB rejected with HTTP 400.
- **Fail-open**: any internal error on `/replace` returns HTTP 200 with
  `{"modified_text": "", "replacements": []}` so the Rust pipeline degrades
  gracefully to T1/T2.

## Impact

- Affected specs: `pii-pipeline` (new SLM sidecar capability)
- Affected code: `packaging/privacyclaw-slm-sidecar` (new file, ~350 lines)
- No Rust source changes required
- Co-deliverable with `update-pii-t3-first-pipeline` task 8.1 (postinstall already
  installs the script file — no postinstall change needed in this change)
- Python runtime required: Python 3.10+ (macOS ships Python 3 since Ventura);
  pip packages: `fastapi`, `uvicorn[standard]`, `httpx`, `pydantic`
