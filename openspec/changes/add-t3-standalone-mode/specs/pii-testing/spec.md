# pii-testing Spec Delta — add-t3-standalone-mode

## ADDED Requirements

### Requirement: T3 Standalone Configuration Validation Tests

Unit tests in `config.rs` SHALL verify that `validate_pii_tiers` accepts `{regex:false, ner:false, slm:true}` as a valid standalone configuration and that previously rejected partial combinations remain rejected.

#### Scenario: T3 standalone combination accepted

- **WHEN** `validate_pii_tiers` is called with `PiiTiersConfig { regex: false, ner: false, slm: true }`
- **THEN** the function returns `Ok(())`

#### Scenario: is_t3_standalone true only for exact standalone combination

- **WHEN** `is_t3_standalone` is called with `{ regex: false, ner: false, slm: true }`
- **THEN** it returns `true`
- **WHEN** called with `{ regex: true, ner: true, slm: true }` (full stack)
- **THEN** it returns `false`
- **WHEN** called with `{ regex: false, ner: false, slm: false }`
- **THEN** it returns `false`

#### Scenario: T2 without T1 still rejected after relaxation

- **WHEN** `validate_pii_tiers` is called with `{ regex: false, ner: true, slm: false }`
- **THEN** the function returns an error containing `"Tier 2 depends on Tier 1"`

#### Scenario: patch_tier3_standalone_allowed integration test

- **WHEN** `ConfigManager::patch` is called with `{ "pii": { "tiers": { "regex": false, "ner": false, "slm": true } } }`
- **THEN** the patch succeeds with `result.ok == true`
- **AND** `mgr.get().await.pii.tiers.slm == true`
- **AND** `mgr.get().await.pii.tiers.regex == false`

---

### Requirement: extract_token_pairs Unit Tests

Unit tests for `extract_token_pairs` SHALL cover: well-formed single span, multiple spans in order, span not found in original (alignment failure), >50% failure abort, unclosed marker skip, and empty input.

#### Scenario: Single span — correct pair extracted

- **WHEN** `extract_token_pairs("hello alice@example.com world", "hello §alice@example.com§ world")` is called
- **THEN** returns `vec![("alice@example.com".to_string(), <synthetic>)]`
- **AND** the synthetic is a non-empty string

#### Scenario: Two spans — both extracted in order

- **WHEN** `extract_token_pairs("Bob called 555-0100", "§Bob§ called §555-0100§")` is called
- **THEN** returns a vec of length 2
- **AND** `pairs[0].0 == "Bob"` and `pairs[1].0 == "555-0100"`

#### Scenario: Alignment failure — span not in original

- **WHEN** `rewritten` contains `§XYZ§` but `"XYZ"` is not present in `original`
- **THEN** that token is skipped (not included in returned pairs)

#### Scenario: >50% failures — partial results with WARN

- **WHEN** `rewritten` contains 6 `§`-wrapped tokens and 4 cannot be found in `original`
- **THEN** only the 2 successfully aligned pairs are returned
- **AND** `tracing::warn!` is emitted

#### Scenario: Unclosed § — skip and continue

- **WHEN** `rewritten = "§alice@example.com, phone: §555-0100§"`
- **THEN** the unclosed first `§` is skipped
- **AND** the correctly closed `§555-0100§` produces one pair

#### Scenario: No § in rewritten — empty vec returned

- **WHEN** `rewritten` contains no `§` character
- **THEN** `extract_token_pairs` returns an empty `Vec`

---

### Requirement: SlmSidecar::detect_and_rewrite Unit Tests

Unit tests for `SlmSidecar::detect_and_rewrite` SHALL use a mock HTTP server to verify correct request construction, response parsing, timeout handling, and `None` return on no-`§` output.

#### Scenario: Correct prompt and max_tokens in request

- **WHEN** `detect_and_rewrite` is called with a 1200-character text
- **THEN** the HTTP POST body sent to the mock server contains `"max_tokens": 512` (`(1200/4 + 128)` = `428`, below the `.max(512)` floor, so final value is `512`)
- **AND** the `messages[0].content` equals `SYSTEM_PROMPT_STANDALONE`
- **AND** the `messages[1].content` equals the input text

#### Scenario: Well-formed response parsed into pairs

- **WHEN** the mock server returns a chat completion with content `"§foo§ and §bar§"`
- **AND** the input text was `"foo and bar"`
- **THEN** `detect_and_rewrite` returns `Some(("§foo§ and §bar§", [("foo", ...), ("bar", ...)]))`

#### Scenario: HTTP timeout returns None

- **WHEN** the mock server does not respond within `timeout_ms`
- **THEN** `detect_and_rewrite` returns `None`

#### Scenario: Non-200 response returns None

- **WHEN** the mock server returns HTTP 500
- **THEN** `detect_and_rewrite` returns `None`

#### Scenario: Response with no § returns None

- **WHEN** the mock server returns a completion with no `§` in the content
- **THEN** `detect_and_rewrite` returns `None`

---

### Requirement: inject_system_instruction Unit Tests

Unit tests for `inject_system_instruction` SHALL cover the Anthropic absent-field case, Anthropic append case, OpenAI append case, OpenAI insert case, and Google no-op case.

#### Scenario: Anthropic absent system field — field created

- **WHEN** `inject_system_instruction` is called on `{}` with `Provider::Anthropic`
- **THEN** the value gains a `"system"` string containing `SYSTEM_REMINDER`
- **AND** the function returns `true`

#### Scenario: Anthropic existing system field — content appended

- **WHEN** the value has `"system": "Be concise."`
- **AND** `inject_system_instruction` is called with `Provider::Anthropic`
- **THEN** `value["system"]` ends with `SYSTEM_REMINDER`
- **AND** the original prefix `"Be concise."` is preserved

#### Scenario: OpenAI existing system message — content appended

- **WHEN** `messages` contains `[{"role":"system","content":"Instructions."}]`
- **AND** `inject_system_instruction` is called with `Provider::OpenAI`
- **THEN** `messages[0]["content"]` ends with `SYSTEM_REMINDER`

#### Scenario: OpenAI no system message — inserted at index 0

- **WHEN** `messages` contains `[{"role":"user","content":"Hello"}]`
- **AND** `inject_system_instruction` is called with `Provider::OpenAI`
- **THEN** `messages[0]["role"] == "system"` and `messages[0]["content"] == SYSTEM_REMINDER`
- **AND** `messages[1]["role"] == "user"`

#### Scenario: Google — returns false, body unchanged

- **WHEN** `inject_system_instruction` is called with `Provider::Google`
- **THEN** returns `false`
- **AND** the `serde_json::Value` passed in is unmodified

---

### Requirement: SidecarProcess Readiness Probe Tests

Unit and integration tests for `SidecarProcess::start` SHALL verify that `--n-predict` is absent from the spawned command, and that the readiness probe correctly handles fast-start and timeout scenarios.

#### Scenario: --n-predict flag absent from command

- **WHEN** `SidecarProcess::start` spawns the child process
- **THEN** the child's command-line arguments do not include the string `"--n-predict"`

#### Scenario: Readiness probe polls /health until 200

- **WHEN** a mock health endpoint returns 503 twice then 200
- **THEN** `start` retries and returns `Ok` after the third poll
- **AND** the total wait time is approximately 500ms (2 × 250ms poll interval)

#### Scenario: Readiness probe timeout kills child

- **WHEN** the mock health endpoint never returns 200 within the configured timeout
- **THEN** `start` returns `Err`
- **AND** the child process has been killed (verified by `child.try_wait()` returning `Ok(Some(_))`)

---

### Requirement: T3 Standalone Pipeline End-to-End Tests

Integration tests SHALL verify the complete standalone flow: PII replaced in forwarded body, synthetic reversed in SSE response, vault persisted, and no-detection fallback.

#### Scenario: End-to-end standalone replacement and reversal

- **WHEN** `PiiPipeline` is constructed with `slm_standalone = true` and a mock `SlmSidecar`
- **AND** the mock sidecar wraps `"Alice"` as `"§Alice§"` in the rewritten text
- **AND** `process_body_t3_standalone` is called
- **THEN** the returned body contains a synthetic token in place of `"Alice"`
- **AND** the vault contains a mapping for `"Alice"`
- **AND** when `ReplacementBuffer::process_delta` processes the synthetic token, it returns `"Alice"`

#### Scenario: No detection — original body returned unchanged

- **WHEN** the mock sidecar returns content with no `§` markers
- **THEN** `process_body_t3_standalone` returns `None`
- **AND** `handle_c2u_pii` forwards the original request body

#### Scenario: System instruction injected after PII replacement

- **WHEN** T3 standalone + Replace mode is active for an Anthropic request
- **AND** `process_body_t3_standalone` succeeds
- **THEN** the forwarded body's `"system"` field contains `SYSTEM_REMINDER`
- **AND** the PII synthetic tokens are also present in the message content

#### Scenario: System instruction injected even when no PII found

- **WHEN** T3 standalone + Replace mode is active for an Anthropic request
- **AND** `process_body_t3_standalone` returns `None` (no § markers from SLM)
- **THEN** `inject_system_instruction_into_body` is still called on the original body
- **AND** the forwarded body contains `SYSTEM_REMINDER` in the `"system"` field
