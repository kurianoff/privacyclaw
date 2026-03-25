# Phase Log: pii-t3-first-pipeline

Feature: Implement Part I of the Adaptive PII Protection: T3-First Pipeline
Branch: feature/pii-t3-first-pipeline

---

=== PHASE HANDOFF ===
Phase:     Design
Status:    complete
Feature:   Implement Part I of the Adaptive PII Protection: T3-First Pipeline
Branch:    feature/pii-t3-first-pipeline
Artifacts:
  .claude/workflow/pii-t3-first-pipeline/design.md
Decisions:
  - Token format: <pii id="TOKEN_ID">DISPLAY_VALUE</pii> for all tiers
  - TOKEN_ID: 8-char base62 from SHA-256(conversation_id + ":" + entity_index); no new crates (sha2 already present)
  - Tier validation rewrite: T3+T1 (no T2) is valid; only T2-without-T1 combinations are invalid
  - slm_standalone field REMOVED from PiiPipeline; expressed entirely by tier matrix
  - /replace endpoint: Python wrapper sidecar; proxy reconstructs modified_text deterministically from replacements array (LLM's modified_text ignored)
  - detect_and_rewrite, extract_token_pairs, SYSTEM_PROMPT_STANDALONE retired (dead code elimination per clippy policy)
  - Buffer: dual-trigger — XML-token path (<pii literal) + existing Aho-Corasick over display values for Level 5
  - Dashboard PiiDetection.synthetic field: bare display_value only (not XML token)
  - System instruction gate: broadened from slm_standalone+Replace to just Replace
  - Proxy reconstructs modified_text right-to-left from sidecar's replacements[] array; LLM's modified_text discarded
  - Entity index pre-assigned in sorted start-offset order before concurrency; eliminates race condition in token ID assignment
  - full_token_to_original HashMap for Level 1; token_id_to_original for Level 2; display_value_to_original for Level 3
  - Level 4: stub (log WARN, pass through) — Part II scope
  - Level 5: existing Aho-Corasick over bare display values unchanged
For next:  Planning phase receives the complete design doc at .claude/workflow/pii-t3-first-pipeline/design.md. Key integration points: vault.rs gains 3 new HashMaps and add_mapping_with_token_id; mod.rs restructures process_request_body_async; buffer.rs gains XML-token holdback path; tier3.rs gains SlmSidecar::replace() and retires detect_and_rewrite. Config validation must be rewritten (existing test suite for slm_standalone will break — tests must be rewritten). Open questions on dashboard WsEvent format and vault backward compat during 24h TTL window are documented but non-blocking for planning.
Open:
  - WsEvent::PiiDetected dashboard display: design resolves this (bare display_value) but implementation detail needs confirmation
  - Vault backward compat: design accepts natural TTL rotation (no migration needed)
  - Sidecar /replace implementation (Python script) is parallel work, not in Rust proxy scope
=== END HANDOFF ===

---

=== PHASE HANDOFF ===
Phase:     Planning
Status:    complete
Feature:   Implement Part I of the Adaptive PII Protection: T3-First Pipeline
Branch:    feature/pii-t3-first-pipeline
Artifacts:
  openspec/changes/update-pii-t3-first-pipeline/proposal.md
  openspec/changes/update-pii-t3-first-pipeline/tasks.md
  openspec/changes/update-pii-t3-first-pipeline/specs/pii-pipeline/spec.md
  openspec/changes/update-pii-t3-first-pipeline/specs/pii-vault/spec.md
Decisions:
  - Change ID: update-pii-t3-first-pipeline (37 tasks, 10 groups)
  - Groups ordered: vault index structures → config validation → tier3 replace/dead-code → pipeline restructure → intercept gate → buffer cascade → token helpers → packaging → doc copy → final validation
  - Dead code retirement (detect_and_rewrite, extract_token_pairs, SYSTEM_PROMPT_STANDALONE, slm_standalone) happens in tasks 3.3, 4.1, 4.2 before pipeline rewrite in 4.3 — clippy-clean at every step
  - Entity index pre-assignment (sorted start-offset order, base_index from vault.mapping_count()) handled in task 4.3 to avoid race in token ID assignment
  - Vault tasks (group 1) precede all other groups because add_mapping_with_token_id is required by pipeline and buffer tasks
  - XML token assembly helper (task 7.1) grouped after buffer to avoid circular dependency concern
  - Packaging (group 8) scoped to postinstall script extension only; Python sidecar is parallel work
  - openspec validate update-pii-t3-first-pipeline --strict passes with no issues
For next:  Development receives change ID update-pii-t3-first-pipeline with 37 ordered tasks. Key risks: (1) vault tasks must land first; (2) config validation rewrite breaks two existing tests — replace them before cargo test; (3) slm_standalone removal causes compile errors in c2u.rs — task 5.1 must follow immediately; (4) Level 4 cascade is a stub only.
Open:      none
=== END HANDOFF ===

---

=== PHASE HANDOFF ===
Phase:     Development
Status:    complete
Feature:   Implement Part I of the Adaptive PII Protection: T3-First Pipeline
Branch:    feature/pii-t3-first-pipeline
Artifacts:
  src/pii/vault.rs
  src/pii/mod.rs
  src/pii/tier3.rs
  src/pii/buffer.rs
  src/config.rs
  src/proxy/intercept/c2u.rs
  tests/t3_standalone_roundtrip.rs
  tests/vault_confidence_test.rs
  packaging/postinstall
  docs/pii-pipeline-v2.md
Decisions:
  - All 10 task groups completed across two interrupted sessions
  - Token format: <pii id="TOKEN_ID">DISPLAY_VALUE</pii> implemented throughout
  - generate_token_id: SHA-256(conv_id:entity_index) first 6 bytes base62 → 8 chars
  - slm_standalone field fully removed; tier matrix is the sole control flow
  - detect_and_rewrite, extract_token_pairs, SYSTEM_PROMPT_STANDALONE retired
  - Buffer: dual-trigger — <pii literal for L1-L4 cascade, Aho-Corasick over display values for L5
  - Level 4 cascade: stub (WARN log, pass through) — Part II scope
  - Dashboard PiiDetection.synthetic: bare display_value only
  - process_body_t3_standalone removed; unified process_request_body_async handles all tier combos
  - Postinstall: sidecar install step added alongside llama-server
  - docs/pii-pipeline-v2.md: design doc moved from .claude/workflow/
  - brew_formula_test failures pre-exist on main; unrelated to this feature
For next:  Testing phase should cover: (1) vault cascade matching L1/L2/L3/L5; (2) tier matrix routing (all 5 valid combos + 2 invalid); (3) T3 /replace integration with mock sidecar; (4) buffer XML-token holdback across SSE chunk boundaries; (5) system instruction injection gating; (6) generate_token_id determinism and uniqueness. Pre-existing brew_formula_test failures are out of scope.
Open:
  - Python sidecar /replace implementation is parallel work (separate session)
  - Level 4 hypothesis matching deferred to Part II (Surface Form Oracle)
=== END HANDOFF ===

---

=== PHASE HANDOFF ===
Phase:     Testing
Status:    complete
Feature:   Implement Part I of the Adaptive PII Protection: T3-First Pipeline
Branch:    feature/pii-t3-first-pipeline
Artifacts:
  tests/t3_standalone_roundtrip.rs
Decisions:
  - 25 integration tests added covering all 6 required areas
  - 385 unit tests continue to pass (0 regressions introduced)
  - brew_formula_test failures (2) pre-exist on main; confirmed by stash check
  - All cascade levels (L1/L2/L3/L5) covered with vault and buffer tests
  - Tier matrix routing tested for all 5 valid combos
  - T3 /replace mock sidecar: success path + HTTP 500 fallback path
  - Buffer SSE chunk-split holdback tested at open-tag boundary
  - generate_token_id: determinism, distinctness, length, charset
  - System instruction: Anthropic injection, idempotency, non-string system field
Open:      none
=== END HANDOFF ===
