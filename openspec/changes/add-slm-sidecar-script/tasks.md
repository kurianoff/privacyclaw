# Tasks: add-slm-sidecar-script

## 1. Scaffold and configuration

- [x] 1.1 Create `packaging/privacyclaw-slm-sidecar` as an executable Python 3
  script (shebang: `#!/usr/bin/env python3`, `chmod 755`). Add the four
  configuration constants parsed from environment variables at module top-level:
  `LLAMA_ENDPOINT` (default `http://127.0.0.1:8080`), `LLAMA_TIMEOUT_S`
  (default `10`, cast to `float`), `SIDECAR_PORT` (default `16442`, cast to
  `int`), `SIDECAR_HOST` (default `127.0.0.1`). Add `LLAMA_SERVER_PATH`
  (default empty string — path to bundled binary, set by Rust or postinstall)
  and `MODEL_PATH` (default empty string — path to GGUF model file).
  Verify: script is importable; constants parse their defaults without error.

- [x] 1.2 Add the dependency guard `__main__` block that imports `fastapi`,
  `uvicorn`, `httpx`, `pydantic` and prints a `pip install` message then exits 1
  if any import fails. Verify: running the script without the deps installed
  exits with code 1 and prints the install command.

## 2. Pydantic models

- [x] 2.1 Define Pydantic v2 models:
  - `ReplaceRequest(BaseModel)`: `text: str`, `conversation_id: str = ""`,
    `entity_start_index: int = 0`.
  - `Replacement(BaseModel)`: `original: str = ""`, `display_value: str = ""`,
    `pii_type: str = ""`, `token_id: str = ""`, `start: int`, `end: int`.
  - `ReplaceResponse(BaseModel)`: `modified_text: str = ""`,
    `replacements: list[Replacement] = []`.
  Verify: `ReplaceResponse()` constructs without arguments and serialises to
  `{"modified_text": "", "replacements": []}`.

## 3. PII type classifier

- [x] 3.1 Implement `classify_pii_type(pii_str: str) -> str` using regex heuristics
  applied in priority order (first match wins):
  1. Email: `\b[\w.+-]+@[\w-]+\.[a-zA-Z]{2,}\b` → `"email"`
  2. SSN: `\b\d{3}-\d{2}-\d{4}\b` → `"ssn"`
  3. Credit card: `\b\d{4}[\s\-]\d{4}[\s\-]\d{4}[\s\-]\d{4}\b` → `"credit_card"`
  4. Phone: `\b[\d\s\-\(\)\.]{7,}\b` with `≥7` digit chars → `"phone"`
  5. API key: `\b[A-Za-z0-9_\-]{20,}\b` with mixed case + digits → `"api_key"`
  6. Person name: two or more title-case words (no digit chars) → `"person_name"`
  7. Fallback → `"other_pii"`
  Verify: `classify_pii_type("test@example.com") == "email"`,
  `classify_pii_type("Anne Nicole") == "person_name"`,
  `classify_pii_type("333-444-5555") == "phone"`.

## 4. Synthetic display value generator

- [x] 4.1 Implement `generate_synthetic(pii_str: str, pii_type: str) -> str`.
  Hash: `h = hashlib.sha256(pii_str.encode()).hexdigest()`. Strategy per type:
  - `person_name`: `h4 = int(h, 16) % 50`; return `NAMES[h4]` from the name pool.
  - `email`: return `f"redacted{h[:4]}@example.com"`.
  - `phone`: return `f"555-000-{int(h[:4], 16) % 10000:04d}"`.
  - `address`: return `f"{int(h[:2], 16) % 90 + 10} Redacted St, Anytown"`.
  - `ssn`: return `f"000-00-{int(h[:4], 16) % 10000:04d}"`.
  - `credit_card`: return `f"4000-0000-0000-{int(h[:4], 16) % 10000:04d}"`.
  - `api_key`: return `f"[REDACTED-KEY-{h[:6]}]"`.
  - `password`: return `"[REDACTED-PASSWORD]"`.
  - `other_pii` (and all other types): return `"[REDACTED]"`.
  Verify: `generate_synthetic("Anne Nicole", "person_name")` returns one of the
  50 NAMES entries and is deterministic (same input → same output every time).

- [x] 4.2 Define the `NAMES` pool as a module-level list of exactly 50 strings, each
  `"FirstName LastName"`. Names must be ethnically diverse and clearly non-real
  people (avoid any name that is an exact real public figure). Representative
  sample required in the list:
  `["Maria Jensen", "Omar Hassan", "Yuki Tanaka", "Sofia Rossi",
    "James Okafor", "Priya Nair", "Lena Müller", "Carlos Reyes",
    "Amara Diallo", "Ethan Kowalski", ...]` — complete to 50 entries.
  Verify: `len(NAMES) == 50` and all entries match `r"^[A-Za-zÀ-ÖØ-öø-ÿ]+ [A-Za-zÀ-ÖØ-öø-ÿ]+$"`.

## 5. Overlap-aware offset resolver

- [x] 5.1 Implement `resolve_replacements(text: str, pii_strings: list[str]) -> list[Replacement]`:
  1. Sort `set(pii_strings)` by length descending (longest first).
  2. Maintain `covered: list[tuple[int, int]] = []`.
  3. For each `pii_str`: call `classify_pii_type` and `generate_synthetic`.
     Use `re.finditer(re.escape(pii_str), text)` to find all occurrences.
     For each match `(s, e)`: skip if any covered `(cs, ce)` satisfies
     `cs <= s < ce or cs < e <= ce`. Otherwise append `(s, e)` to `covered`
     and append a `Replacement` to `results`.
  4. Sort `results` by `start` ascending.
  5. Return `results`.
  Verify (unit): given text `"Anne Nicole called Anne about 333-444-5555"` and
  `pii_strings=["Anne Nicole", "Anne", "333-444-5555"]`, the result has 3
  entries: "Anne Nicole" at correct offset, "Anne" (the second occurrence only,
  not overlapping with "Anne Nicole"), and the phone number. Result is sorted by
  `start`.

## 6. LLM call helper and response parser

- [x] 6.1 Implement `async def call_llm_for_pii(text: str) -> list[str]` using
  `httpx.AsyncClient`. POST to `f"{LLAMA_ENDPOINT}/v1/chat/completions"` with:

  ```json
  {
    "model": "local",
    "messages": [
      {"role": "system", "content": "<system prompt from design §4.3>"},
      {"role": "user",   "content": "Text: <text>"}
    ],
    "max_tokens": 512,
    "temperature": 0.0
  }
  ```

  Timeout: `LLAMA_TIMEOUT_S`. Parse response: extract
  `choices[0].message.content`, strip whitespace, strip markdown fences
  (`` ```json `` / ` ``` `), `json.loads()`. Validate result is a list; filter
  to string elements only. On any exception (`httpx.TimeoutException`,
  `httpx.ConnectError`, `json.JSONDecodeError`, `ValueError`, or other): log
  `WARNING` and return `[]`.
  Verify: `call_llm_for_pii` called against a mock server returning
  `{"choices":[{"message":{"content":"[\"Anne\",\"333-444-5555\"]"}}]}` returns
  `["Anne", "333-444-5555"]`.

- [x] 6.2 Verify markdown fence stripping: if LLM returns
  ` ```json\n["Alice"]\n``` ` the parser returns `["Alice"]`. If LLM returns
  prose with no JSON array, parser returns `[]`.

## 7. FastAPI endpoints

- [x] 7.1 Implement `GET /health` endpoint:
  - Returns `{"status": "ok"}` (HTTP 200) only after llama-server has passed its
    own health check at least once since sidecar startup.
  - Returns `{"status": "starting"}` (HTTP 503) if llama-server is not yet ready.
  Verify: before llama-server is ready, `/health` returns 503; after, returns 200.

- [x] 7.2 Implement `POST /replace` endpoint with fail-open guarantee:

  ```python
  @app.post("/replace")
  async def replace(req: ReplaceRequest) -> ReplaceResponse:
      if len(req.text) > 32_768:
          raise HTTPException(status_code=400, detail="text too large")
      try:
          pii_strings = await call_llm_for_pii(req.text)
          replacements = resolve_replacements(req.text, pii_strings)
          return ReplaceResponse(replacements=replacements)
      except Exception as exc:
          log.warning("SLM /replace failed: %s", exc)
          return ReplaceResponse(replacements=[])
  ```

  Verify: text of 32,769 chars returns HTTP 400. Valid request with llama-server
  returning `["Alice"]` returns `ReplaceResponse` with one entry. LLM failure
  returns HTTP 200 with empty `replacements`.

- [x] 7.3 Implement `POST /v1/chat/completions` streaming-aware proxy pass-through:
  - Read raw request body with `await request.body()`.
  - Check if the request body contains `"stream": true` (after JSON parse).
  - If streaming: use `httpx.AsyncClient.stream("POST", ...)` and return a
    `StreamingResponse` that yields chunks from llama-server as they arrive.
    Media type: `text/event-stream`.
  - If non-streaming: use `httpx.AsyncClient.post(...)` and return a plain
    `Response` with llama-server's response body and status code.
  - On `httpx.TimeoutException` or `httpx.ConnectError`: return HTTP 503.
  - Pass `Content-Type: application/json` header to llama-server; do not forward
    other client headers to the internal server.
  Verify: non-streaming request proxied and response body returned unchanged.
  Streaming request: `StreamingResponse` with `text/event-stream` media type
  returned (mock llama-server sends two `data:` chunks).

## 8. llama-server subprocess manager

- [x] 8.1 Implement `LlamaServerManager` class (or module-level functions) that:
  - On FastAPI startup (`@app.on_event("startup")`): if `LLAMA_SERVER_PATH` and
    `MODEL_PATH` are non-empty, validate both paths exist and fail-loud with a
    clear error message + `sys.exit(1)` if either is missing.
    Spawn llama-server using `asyncio.create_subprocess_exec` (not
    `subprocess.Popen` — avoids blocking the asyncio event loop):

    ```text
    [LLAMA_SERVER_PATH, "--model", MODEL_PATH, "--port", <internal_port>,
     "--ctx-size", "2048", "--log-disable"]
    ```

    where `<internal_port>` is extracted from `LLAMA_ENDPOINT` using
    `urllib.parse.urlparse(LLAMA_ENDPOINT).port or 8080`.
  - Starts a background asyncio task (`asyncio.create_task`) that:
    (a) health-polls `GET {LLAMA_ENDPOINT}/health` every 5 seconds via
        `httpx.AsyncClient` and updates a module-level `_llama_ready: bool` flag.
    (b) also checks whether the subprocess has exited via `process.returncode is not None`
        on each poll cycle. On subprocess exit: log WARN with exit code, then restart.
  - On failed health poll (3 consecutive failures after initial readiness): kill
    the subprocess and restart it. Use exponential backoff: 5 s, 10 s, 20 s, max
    40 s between restart attempts.
  - Tracks whether llama-server is ready in module-level `_llama_ready: bool`
    (used by `/health` endpoint task 7.1).
  - On FastAPI shutdown (`@app.on_event("shutdown")`): cancel the watcher task,
    send `SIGTERM` to the subprocess via `process.terminate()`, await
    `process.wait()` with a 5 s timeout, then `process.kill()` if still running.
  Verify: if `LLAMA_SERVER_PATH` is empty, no subprocess is spawned and the
  sidecar starts normally with `_llama_ready = True` (pass-through mode).
  Verify: if `LLAMA_SERVER_PATH` points to a non-existent file, sidecar exits 1
  with a clear error before binding any port.

- [x] 8.2 Log subprocess lifecycle events at appropriate levels per CLAUDE.md
  logging rules:
  - `WARN`: llama-server started (with PID and port), stopped, restarted.
  - `WARN`: llama-server became ready (elapsed ms).
  - `WARN`: llama-server crashed (with exit code), restart scheduled.
  - `DEBUG`: each health-poll attempt and result.
  Never log model file path at INFO or above (may contain user home directory).

## 9. Logging and privacy

- [x] 9.1 Set up structured logging using Python's `logging` module with a format
  that emits key=value fields compatible with the tracing convention in the rest
  of the project:
  `"%(asctime)s [%(levelname)s] %(name)s %(message)s"`.
  Log level controlled by `LOG_LEVEL` env var (default `INFO`).

- [x] 9.2 Enforce privacy in all log calls:
  - Raw `text` content from `ReplaceRequest`: NEVER logged at INFO or above.
  - `conversation_id`: logged at DEBUG only.
  - LLM response content: logged at DEBUG only, truncated to 256 chars.
  - `LLAMA_ENDPOINT`, `SIDECAR_PORT`, `SIDECAR_HOST`: safe to log at INFO.
  Verify: no `log.info` or `log.warning` call in the codebase references `req.text`
  or `conversation_id`.

## 10. End-to-end integration test

- [x] 10.1 Add `tests/test_slm_sidecar.py` (pytest). Use `httpx.AsyncClient` with
  the sidecar started in-process (import the module and use FastAPI `TestClient`
  or `AsyncClient` with `ASGITransport`). Mock llama-server with a minimal
  `httpx.MockTransport` or `respx` fixture. Test cases:
  - `test_health_returns_ok`: `/health` returns 200 when llama-server mock is up.
  - `test_replace_success`: mock LLM returns `["Anne Nicole"]`; verify response
    has one replacement with `pii_type="person_name"` and non-empty `display_value`.
  - `test_replace_text_too_large`: text of 32,769 chars → HTTP 400.
  - `test_replace_llm_timeout`: mock LLM times out → HTTP 200 with empty list.
  - `test_replace_overlap_deduplication`: mock LLM returns `["Anne Nicole", "Anne"]`;
    text is `"Anne Nicole"` — verify only one replacement (not two overlapping).
  - `test_chat_completions_nonstreaming`: request without `"stream": true` →
    response body from mock llama-server returned unchanged.
  - `test_chat_completions_streaming`: request with `"stream": true` →
    `text/event-stream` response with streamed chunks.

## 11. Packaging validation

- [x] 11.1 Verify `packaging/postinstall` already installs the sidecar script (task 8.1
  of `update-pii-t3-first-pipeline`). If that task is not yet complete, note the
  dependency but do not duplicate the postinstall change in this task list.
  The sidecar script file must be present at the path the postinstall script
  expects: `$SHARE_DIR/privacyclaw-slm-sidecar`.

- [x] 11.2 Verify the script is executable (`chmod 755`) and has the correct shebang
  (`#!/usr/bin/env python3`). Run `python3 -c "import ast; ast.parse(open('packaging/privacyclaw-slm-sidecar').read())"` to confirm it is valid Python syntax.

## 12. Final validation

- [x] 12.1 Run `python3 -m py_compile packaging/privacyclaw-slm-sidecar` — must
  complete with no errors.

- [x] 12.2 Run `pytest tests/test_slm_sidecar.py -v` — all tests in task 10.1 must pass.

- [x] 12.3 Run `openspec validate add-slm-sidecar-script --strict` — must pass with
  no issues.

- [x] 12.4 Manual smoke test (optional, if llama.cpp is available locally):
  Start the sidecar with `LLAMA_SERVER_PATH=<path> MODEL_PATH=<model.gguf>
  python3 packaging/privacyclaw-slm-sidecar`. Confirm `/health` returns 200 and
  `POST /replace` with a test text returns non-empty replacements.
