# Tasks: add-t3-standalone-mode

## 1. Config changes

- [x] 1.1 In `config.rs`, relax `validate_pii_tiers`: allow `{regex:false, ner:false, slm:true}` — add an explicit early-return `Ok(())` branch before the existing `slm` guard.
- [x] 1.2 Add `pub(crate) fn is_t3_standalone(tiers: &PiiTiersConfig) -> bool` returning `!tiers.regex && !tiers.ner && tiers.slm`.
- [x] 1.3 Update test `patch_tier3_without_tier2_is_error`: change the patch to `{regex:false, ner:true, slm:true}` so it still exercises the partial-dependency rejection, not the standalone case.
- [x] 1.4 Add test `patch_tier3_standalone_allowed`: call `mgr.patch({pii:{tiers:{regex:false,ner:false,slm:true}}})` and assert `result.ok == true` plus the tiers values are stored.
- [x] 1.5 Add unit test `is_t3_standalone_true_for_standalone_combo` and `is_t3_standalone_false_for_full_stack`.

## 2. Sidecar process changes

- [x] 2.1 In `pii/tier3.rs`, remove `--n-predict` / `256` argument pair from `SidecarProcess::start` (two `.arg()` calls to delete).
- [x] 2.2 Add a configurable `readiness_timeout_secs: u64` parameter to `SidecarProcess::start` (add a new `start_with_options` variant or add the parameter directly — use `u64` with default 30).
- [x] 2.3 Implement the readiness probe loop in `SidecarProcess::start`: poll the sidecar port at 100ms intervals until a TCP connection succeeds or timeout. On timeout, log `WARN` and return `Ok` anyway (fail-open — proxy starts in a degraded state rather than refusing to start; T3 calls will return `None` and requests will be forwarded unredacted).
- [x] 2.4 Emit `tracing::warn!` at probe start (with `pid`, `port`, `model`) and at probe completion (with elapsed ms).
- [x] 2.5 Add unit test `sidecar_n_predict_absent`: construct the `Command` args list and assert `--n-predict` is not in it. (Test the command builder logic directly without spawning a real process.)

## 3. T3 tier new method

- [x] 3.1 Add `const SYSTEM_PROMPT_STANDALONE: &str` in `pii/tier3.rs`. Content: instructs the SLM to identify PII spans and rewrite the text with `§value§` wrappers around each detected PII string, using the exact original substring inside the markers.
- [x] 3.2 Implement `fn extract_token_pairs(original: &str, rewritten: &str) -> Vec<(String, String)>` in `pii/tier3.rs`:
  - Scan `rewritten` for `§` markers.
  - For each `§...§` pair, find the inner text in `original` (searching forward from the last match position).
  - On find: record `(original_span, inner_text)` pair. (`inner_text` is used as the vault key.)
  - On not-find: increment failure counter, skip token.
  - After all tokens: if `failures > total / 2`, emit `tracing::warn!` with counts.
  - Return all successfully aligned pairs (partial results on abort).
  - Unclosed `§` (no matching close): skip and continue.
  - No `§` at all: return empty vec immediately.
- [x] 3.3 Implement `async fn detect_and_rewrite(&self, text: &str) -> Option<(String, Vec<(String, String)>)>` on `SlmSidecar`:
  - Compute `max_tokens = ((text.len() as u32 / 4) + 128).max(512).min(4096)`.
  - Build `ChatCompletionRequest` with `SYSTEM_PROMPT_STANDALONE` as system message and `text` as user message.
  - POST to `{endpoint}/v1/chat/completions` with `max_tokens`.
  - On timeout or non-200: log `WARN`, return `None`.
  - Extract `choices[0].message.content` as `rewritten`.
  - If `rewritten` contains no `§`: log `WARN "Tier3: SLM produced no § markers"`, return `None`.
  - Call `extract_token_pairs(text, &rewritten)`.
  - Return `Some((rewritten, pairs))`.
- [x] 3.4 Add unit tests for `extract_token_pairs` covering all six scenarios from the spec delta (single span, two spans, alignment failure, >50% abort, unclosed marker, no `§`).
- [x] 3.5 Add unit tests for `detect_and_rewrite` using `wiremock` or `mockito` mock HTTP server (correct request structure, well-formed response, timeout, non-200, no `§`).

## 4. Pipeline changes

- [x] 4.1 In `pii/mod.rs`, add `pub slm_standalone: bool` field to `PiiPipeline`.
- [x] 4.2 In `PiiPipeline::tier1_only`, set `slm_standalone: false`.
- [x] 4.3 In `PiiPipeline::new`, set `slm_standalone: crate::config::is_t3_standalone(&cfg.tiers)`.
- [x] 4.4 Implement private async fn `process_body_t3_standalone(&self, body: &[u8], vault_handle: &VaultHandle, provider: Provider) -> Option<(Vec<u8>, Vec<PiiDetection>)>`:
  - Parse body as JSON.
  - Extract message content strings (same `messages_field` logic as `process_request_body_async`).
  - For each text: call `self.slm.as_ref()?.detect_and_rewrite(text).await`.
  - On `None` result: skip text (leave unchanged).
  - On `Some((rewritten, pairs))`: for each `(original_span, inner)` pair where `inner == original_span`, call `vault.add_mapping(original_span.clone(), format!("§{}§", original_span), PiiType::Unknown, 3)`. Do NOT call `vault.get_or_create` — that generates a random synthetic name. The vault key must be `§Peter§` (with delimiters) so the inbound `ReplacementBuffer` can match it in the LLM response. Use the already-rewritten text from the SLM response as the body replacement (it already contains `§Peter§`).
  - Reassemble modified JSON value and serialize back to bytes.
  - Return `Some((bytes, detections))`.
- [x] 4.5 Add `pub const SYSTEM_REMINDER: &str` in `pii/mod.rs`.
- [x] 4.6 Implement `pub fn inject_system_instruction(value: &mut serde_json::Value, provider: Provider) -> bool` in `pii/mod.rs`:
  - `Provider::Anthropic`: match `value["system"]` as mutable string; append; create if missing; return `false` + `WARN` if not a string type.
  - `Provider::OpenAI`: find first `role=system` in `value["messages"]` array; append to content; or insert new system message at index 0. Return `false` + `WARN` if `messages` absent.
  - `Provider::Google`: log `DEBUG "Tier3: skipping system instruction for Google provider"`, return `false`.
  - Other providers: return `false`.
- [x] 4.7 In `process_request_body_async`, add fast-path at top: `if self.slm_standalone { return self.process_body_t3_standalone(...).await; }`.
- [x] 4.8 Add unit tests for `inject_system_instruction` (5 scenarios from spec delta).
- [x] 4.9 Add unit test for `PiiPipeline::new` with standalone tiers config: assert `pipeline.slm_standalone == true` and `pipeline.tier2.is_none()`.

## 5. Intercept changes

- [x] 5.1 In `proxy/intercept.rs`, add private fn `inject_system_instruction_into_body(body: &[u8], provider: Provider) -> Option<Vec<u8>>`:
  - Parse body as JSON.
  - Call `pii::inject_system_instruction(&mut value, provider)`.
  - If returns `true`: serialize value back to bytes, return `Some(bytes)`.
  - If returns `false`: return `None`.
- [x] 5.2 In `handle_c2u_pii`, restructure the `forward_request` construction into two sequential stages:
  - Stage 1: Run PII pipeline via `process_request_body_async` (or `process_body_t3_standalone` via the pipeline's fast-path). Working body = result or original.
  - Stage 2: If `pii.pipeline.slm_standalone && mode == Replace`: call `inject_system_instruction_into_body(working_body, provider)` and use the result if `Some`, otherwise keep working body.
- [x] 5.3 Ensure stage 2 runs regardless of stage 1 outcome (i.e., system instruction is injected even when no PII was detected by the SLM).
- [x] 5.4 Verify `Content-Length` header is updated after stage 2 (the existing `rebuild_request_with_content_length` call covers this if stages feed into it correctly).

## 6. Tests

- [x] 6.1 Unit: `config::tests::patch_tier3_standalone_allowed` (see task 1.4).
- [x] 6.2 Unit: `config::tests::is_t3_standalone_true` and `is_t3_standalone_false` (see task 1.5).
- [x] 6.3 Unit: `pii::tier3::tests::extract_token_pairs_*` (6 cases, see task 3.4).
- [x] 6.4 Unit: `pii::tier3::tests::detect_and_rewrite_*` (5 cases, see task 3.5).
- [x] 6.5 Unit: `pii::tests::inject_system_instruction_*` (5 cases, see task 4.8).
- [x] 6.6 Unit: `pii::tests::pipeline_slm_standalone_flag` (see task 4.9).
- [x] 6.7 Integration: `tests/integration/t3_standalone_roundtrip_test.rs` — full flow with mock SLM server:
  - Construct `PiiPipeline` with `slm_standalone = true` and a `MockSlmSidecar`.
  - Call `process_request_body_async` on a body containing `"Alice"`.
  - Assert returned body contains a synthetic token (not `"Alice"`).
  - Assert vault contains the mapping.
  - Feed the synthetic token through `ReplacementBuffer::process_delta` and assert `"Alice"` is restored.
- [x] 6.8 Integration: `tests/integration/t3_standalone_no_detection_test.rs` — SLM returns no `§` markers:
  - Assert `process_request_body_async` returns `None`.
  - Assert `handle_c2u_pii` still injects `SYSTEM_REMINDER` into the forwarded body.
- [ ] 6.9 Run `cargo test` and fix all compilation errors and test failures before marking this complete.
- [ ] 6.10 Run `cargo clippy -- -D warnings` and fix all warnings.
