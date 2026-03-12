# Change: Add Tier 3 Standalone Mode

## Why

Tier 3 (the local SLM sidecar) currently requires Tier 1 (regex) and Tier 2 (GLiNER NER) to be enabled as prerequisites. This forces users who want LLM-assisted PII detection — particularly for unstructured freeform text, proper nouns, or domain-specific entities not covered by regex — to also install a GLiNER model binary. GLiNER is a heavy native dependency that many deployment environments cannot support (no GPU, strict sandboxing, corporate binary policy).

The dependency is also architecturally incorrect: Tier 3 in standalone mode does not disambiguate Tier 2 candidates — it performs its own independent rewrite of the full message body. The `disambiguate` method was designed for a different role. Standalone mode needs a separate `detect_and_rewrite` path that sends the full text to the SLM and extracts replacement pairs from the rewritten output.

Allowing `{regex: false, ner: false, slm: true}` as a valid standalone configuration unblocks:

1. Environments that have a local llama-server but cannot run GLiNER.
2. Privacy use cases where context-aware PII detection (e.g., character names used as identifiers, codenames, project-specific tokens) is more important than structured entity coverage.
3. Future model management (add-macos-packaging) where the bundled GGUF model is used as the primary detection path.

## What Changes

### `config.rs`
- `validate_pii_tiers` relaxed: `{regex:false, ner:false, slm:true}` is explicitly allowed as a standalone mode.
- New `pub(crate) fn is_t3_standalone(tiers: &PiiTiersConfig) -> bool` — returns `true` when `!regex && !ner && slm`.
- Test `patch_tier3_without_tier2_is_error` updated to remain valid for the `{regex:false, ner:true, slm:true}` case only.
- New test `patch_tier3_standalone_allowed` verifies the new standalone combination passes validation.

### `pii/tier3.rs`
- New async method `SlmSidecar::detect_and_rewrite(&self, text: &str) -> Option<(String, Vec<(String, String)>)>` — sends text with `SYSTEM_PROMPT_STANDALONE`, receives rewritten text, extracts original→synthetic pairs.
- New `const SYSTEM_PROMPT_STANDALONE: &str` — instructs the SLM to rewrite text with `§value§` wrappers around each detected PII value (exact original substring inside the markers).
- New `fn extract_token_pairs(original: &str, rewritten: &str) -> Vec<(String, String)>` — char-by-char diff that emits `(original_span, synthetic_span)` pairs for each `§...§` region; aborts and returns partial results if >50% of tokens fail alignment or the output contains no `§` markers.
- `SidecarProcess::start`: remove `--n-predict 256` argument (it prevents per-request `max_tokens` from taking effect on llama-server).
- `SidecarProcess::start`: add readiness probe — poll `GET /health` with configurable timeout before returning; log `WARN` on slow start.
- Dynamic `max_tokens` in `detect_and_rewrite`: `((text.len() as u32 / 4) + 128).max(512).min(4096)`.

### `pii/mod.rs`
- `PiiPipeline` struct: add `pub slm_standalone: bool`.
- `PiiPipeline::new`: set `slm_standalone` from `is_t3_standalone(&cfg.tiers)`.
- `PiiPipeline::tier1_only`: set `slm_standalone: false`.
- `process_request_body_async`: add fast-path branch for standalone mode that calls new `process_body_t3_standalone`.
- New private async fn `process_body_t3_standalone` — calls `detect_and_rewrite`, populates vault from returned pairs, returns modified body.
- New `pub fn inject_system_instruction(value: &mut serde_json::Value, provider: Provider) -> bool`:
  - Anthropic: append `\n\n<system-reminder>\n{SYSTEM_REMINDER}\n</system-reminder>` to top-level `system` string field (create field if absent).
  - OpenAI: append `SYSTEM_REMINDER` to the content of the first `{"role":"system"}` message; if none exists, insert one at index 0.
  - Google: no-op, returns `false`, logs `DEBUG` (Google `systemInstruction` schema is incompatible; system instruction cannot be delivered).
- New `pub const SYSTEM_REMINDER: &str` — text telling the LLM to treat `§value§` tokens as opaque literals and reproduce them verbatim.

### `proxy/intercept.rs`
- `handle_c2u_pii`: restructure `forward_request` construction into two sequential stages:
  1. Run PII pipeline (may return `None` → use original body).
  2. If `T3 standalone && Replace mode`: call `inject_system_instruction_into_body` on the working body regardless of stage 1 result.
- New private fn `inject_system_instruction_into_body(body: &[u8], provider: Provider) -> Option<Vec<u8>>`.

### No changes needed
`pii/vault.rs`, `pii/buffer.rs`, `pii/synth.rs` — `§` is already a non-alphanumeric character and will not trigger `synthetic_key_first_chars()` matching unless a `§`-prefixed synthetic is explicitly added, which it is not in standalone mode. The vault stores the original→synthetic pairs emitted by `extract_token_pairs`.

## Impact

### Breaking changes
None. The default configuration (`{regex:true, ner:false, slm:false}`) is unchanged. Existing Tier 1+2+3 users are unaffected.

### Conflicting changes
- `add-macos-packaging` (137/163 tasks): uses `slm` config fields. No structural conflicts; both changes read the same `PiiConfig.slm` fields.
- `refactor-maintainability` (0/57 tasks): touches `intercept.rs`. The two-stage `forward_request` construction in stage 5 must be written after or alongside the `InterceptContext` extraction to avoid a rebase conflict. Coordinate the intercept changes.

### Security considerations
- The system instruction injected into the forwarded request contains no PII — it references only `§value§` as a syntactic marker.
- The SLM response is never forwarded to the client. Only the extracted `(original, synthetic)` pairs are stored in the vault.
- If `extract_token_pairs` aborts (>50% misalignment), the pipeline falls back to no-replacement and logs a `WARN`. No PII is leaked; the request is forwarded with the original body.
- The standalone mode has no Tier 1 regex pass. Structured PII (API keys, SSNs, credit cards) is not guaranteed to be detected unless the SLM independently recognises them. Users who need structured PII coverage should use Tier 1 + Tier 3 (which is not a currently supported combination; a follow-on change may add it).
