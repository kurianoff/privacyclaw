# pii-pipeline Spec Delta — add-t3-standalone-mode

## MODIFIED Requirements

### Requirement: Tiered PII Detection Pipeline

The system SHALL implement a three-tier PII detection pipeline that runs on the fully-buffered outbound request body. All three tiers operate in sequence on the same text. Later tiers SHALL skip spans already detected by earlier tiers. The pipeline SHALL be configurable: Tier 1 is always active when PII mode is enabled (except in T3 standalone mode); Tiers 2 and 3 are independently opt-in. The combination `{regex:false, ner:false, slm:true}` SHALL be a valid standalone configuration.

#### Scenario: T3 standalone — valid configuration accepted

- **WHEN** `pii.tiers.regex = false` and `pii.tiers.ner = false` and `pii.tiers.slm = true`
- **THEN** `validate_pii_tiers` returns `Ok(())`
- **AND** `is_t3_standalone(&tiers)` returns `true`

#### Scenario: T2 without T1 still rejected

- **WHEN** `pii.tiers.regex = false` and `pii.tiers.ner = true`
- **THEN** `validate_pii_tiers` returns an error containing `"Tier 2 depends on Tier 1"`

#### Scenario: T3 with T2 but without T1 still rejected

- **WHEN** `pii.tiers.regex = false` and `pii.tiers.ner = true` and `pii.tiers.slm = true`
- **THEN** `validate_pii_tiers` returns an error (mixed partial dependency is invalid)

---

## ADDED Requirements

### Requirement: T3 Standalone Detection and Rewrite

When `PiiPipeline::slm_standalone` is `true` and PII mode is `"replace"`, the pipeline SHALL skip Tier 1 and Tier 2 entirely and invoke `SlmSidecar::detect_and_rewrite` on each message content string. The SLM SHALL receive the full text with `SYSTEM_PROMPT_STANDALONE`. The response SHALL be parsed by `extract_token_pairs` to produce `(original_span, synthetic)` pairs. These pairs SHALL be inserted into the vault via `get_or_create`. The modified body (with synthetic tokens) SHALL be forwarded to the upstream LLM API.

If `detect_and_rewrite` returns `None` (timeout, HTTP error, or no `§` markers), the original body MUST be forwarded unchanged without error. A `WARN` log entry SHALL be emitted.

#### Scenario: SLM wraps PII — vault populated and body modified

- **WHEN** `slm_standalone = true` and `pii.mode = "replace"`
- **AND** the message text is `"Call John Smith at 555-0100"`
- **AND** the SLM returns `"Call §John Smith§ at §555-0100§"`
- **THEN** `extract_token_pairs` returns `[("John Smith", <synthetic1>), ("555-0100", <synthetic2>)]`
- **AND** the vault contains both mappings
- **AND** the forwarded body contains the synthetic tokens in place of the originals

#### Scenario: SLM returns no § markers — original body forwarded

- **WHEN** `slm_standalone = true` and `pii.mode = "replace"`
- **AND** the SLM response contains no `§` characters
- **THEN** `detect_and_rewrite` returns `None`
- **AND** the original body is forwarded unchanged
- **AND** a `WARN` log entry is emitted

#### Scenario: SLM timeout — original body forwarded

- **WHEN** the SLM sidecar HTTP call exceeds `slm.timeout_ms`
- **THEN** `detect_and_rewrite` returns `None`
- **AND** the original body is forwarded unchanged
- **AND** a `WARN` log entry is emitted

#### Scenario: Multi-turn — same span maps to same synthetic

- **WHEN** `slm_standalone = true` and a multi-turn conversation sends `"John Smith"` in turn 1 and turn 3
- **AND** both turns are processed against the same vault
- **THEN** both forwarded bodies contain the same synthetic token for `"John Smith"`
- **AND** `vault.get_or_create("John Smith", PiiType::Unknown)` returns the same value both times

---

### Requirement: extract_token_pairs Algorithm

`fn extract_token_pairs(original: &str, rewritten: &str) -> Vec<(String, String)>` SHALL parse `§`-wrapped spans in `rewritten`, locate each span in `original`, and return `(original_span, synthetic)` pairs. The function SHALL abort with partial results when more than 50% of identified `§` token markers fail alignment against `original`. An unclosed `§` marker SHALL be skipped without aborting. A span not found in `original` SHALL increment the failure counter.

#### Scenario: Well-formed rewrite produces correct pairs

- **WHEN** `original = "email me at alice@corp.com tomorrow"`
- **AND** `rewritten = "email me at §alice@corp.com§ tomorrow"`
- **THEN** `extract_token_pairs` returns exactly one pair: `("alice@corp.com", <synthetic>)`

#### Scenario: Multiple spans extracted in order

- **WHEN** `original = "I am Bob, my phone is 555-0199"`
- **AND** `rewritten = "I am §Bob§, my phone is §555-0199§"`
- **THEN** two pairs are returned: `("Bob", <s1>)` and `("555-0199", <s2>)`

#### Scenario: >50% alignment failure returns partial results

- **WHEN** `original = "foo bar baz"`
- **AND** `rewritten` contains 4 `§`-wrapped tokens, 3 of which cannot be found in `original`
- **THEN** `extract_token_pairs` returns only the 1 successfully aligned pair
- **AND** a `WARN` log entry is emitted noting the partial alignment

#### Scenario: Unclosed § marker skipped

- **WHEN** `rewritten = "§Alice Brown, contact: §555-0199§"`
- **THEN** the unclosed first `§` is skipped
- **AND** the correctly closed `§555-0199§` produces one pair

---

### Requirement: System Instruction Injection

When T3 standalone mode is active and PII mode is `"replace"`, the proxy SHALL inject `SYSTEM_REMINDER` into the forwarded request body after PII replacement. The injection SHALL be provider-specific.

For Anthropic: the content SHALL be appended to the top-level `system` string field (created if absent) using the format `\n\n<system-reminder>\n{SYSTEM_REMINDER}\n</system-reminder>`.

For OpenAI: the content SHALL be appended to the first `{"role":"system"}` message content, or a new system message SHALL be inserted at index 0 if none exists.

For Google: injection SHALL be skipped and `inject_system_instruction` SHALL return `false`. A `DEBUG` log entry SHALL note the skip.

`inject_system_instruction` SHALL return `true` when injection succeeded and `false` otherwise. If the JSON structure does not match expectations (e.g., Anthropic `system` is not a string), the function SHALL return `false` and emit a `WARN`.

#### Scenario: Anthropic system field created when absent

- **WHEN** the request body has no `"system"` field
- **AND** `inject_system_instruction(&mut value, Provider::Anthropic)` is called
- **THEN** the body gains `"system": "<system-reminder>\n{SYSTEM_REMINDER}\n</system-reminder>"`
- **AND** the function returns `true`

#### Scenario: Anthropic system field appended when present

- **WHEN** the request body has `"system": "You are a helpful assistant."`
- **AND** `inject_system_instruction(&mut value, Provider::Anthropic)` is called
- **THEN** the `system` field becomes `"You are a helpful assistant.\n\n<system-reminder>\n{SYSTEM_REMINDER}\n</system-reminder>"`

#### Scenario: OpenAI — system message appended

- **WHEN** the `messages` array contains a `{"role":"system","content":"Be concise."}` entry
- **AND** `inject_system_instruction(&mut value, Provider::OpenAI)` is called
- **THEN** the system message content becomes `"Be concise.\n\n{SYSTEM_REMINDER}"`

#### Scenario: OpenAI — no system message — one inserted at index 0

- **WHEN** the `messages` array contains only `user` and `assistant` messages
- **AND** `inject_system_instruction(&mut value, Provider::OpenAI)` is called
- **THEN** a new `{"role":"system","content":"{SYSTEM_REMINDER}"}` is inserted at index 0

#### Scenario: Google — injection skipped

- **WHEN** `inject_system_instruction(&mut value, Provider::Google)` is called
- **THEN** the function returns `false`
- **AND** the request body is not modified

---

### Requirement: SidecarProcess Readiness and max_tokens

`SidecarProcess::start` SHALL remove the `--n-predict` argument from the llama-server command so that per-request `max_tokens` values take effect. After spawning the process, `start` SHALL poll `GET /health` on the sidecar endpoint at 250ms intervals until the server responds with HTTP 200, timing out after a configurable duration (default: 30 seconds). If the health check times out, `start` SHALL return an `Err`. A `WARN` log entry SHALL be emitted at the start and completion of the readiness probe.

`SlmSidecar::detect_and_rewrite` SHALL compute `max_tokens` dynamically as `((text.len() as u32 / 4) + 128).max(512).min(4096)`.

#### Scenario: --n-predict removed from sidecar command

- **WHEN** `SidecarProcess::start` is called
- **THEN** the spawned llama-server process command line does not include `--n-predict`
- **AND** a per-request `max_tokens` field in the chat completion request body is honoured

#### Scenario: Readiness probe succeeds before timeout

- **WHEN** llama-server becomes ready within 30 seconds
- **AND** `GET /health` returns HTTP 200
- **THEN** `SidecarProcess::start` returns `Ok(process)`
- **AND** a `WARN` log entry notes the startup time

#### Scenario: Readiness probe times out

- **WHEN** `GET /health` does not return HTTP 200 within the configured timeout
- **THEN** `SidecarProcess::start` returns `Err`
- **AND** the child process is killed before returning

#### Scenario: Dynamic max_tokens proportional to input

- **WHEN** `detect_and_rewrite` is called with a 2000-character text
- **THEN** the `max_tokens` field in the chat completion request equals `((2000 / 4) + 128).max(512).min(4096)` = `628`
- **WHEN** called with a 200-character text
- **THEN** `max_tokens` equals `512` (minimum floor applied)
