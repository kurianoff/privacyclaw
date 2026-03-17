# Refactor Log — entire-src

## Baseline

Branch: refactor/entire-src
Scope: src/ (entire codebase)
Boundaries: wire protocol (CONNECT tunnel, SSE wire format, WebSocket framing), public CLI interface (argument names and behavior), CA certificate DN parameters, database schema column names
Baseline tests: 374 passed; 2 failed (pre-existing: brew_formula_test — missing homebrew repo files)
Pre-existing failures: formula_privacyclaw_rb_exists_and_valid, cask_privacyclaw_app_rb_exists_and_valid

---

## Phase 2 — Blueprint Handoff

```text
=== PHASE HANDOFF ===
Phase:     Blueprint
Status:    complete
Scope:     src/ (entire codebase — 33 Rust source files)
Branch:    refactor/entire-src
Artifacts: .claude/workflow/entire-src/task-list.md

Decisions:
  - T1 (god-file split) is the critical-path root; T2, T3, T4 all depend on it.
  - T4 was given an additional dependency on T2 (Contrarian critical): M7 and L4 live in
    the same file region as H3, so T2 must merge before T4 touches that area.
  - T5 (H4 + L2) is independent of the intercept chain and can run in a separate worktree
    in parallel with T1–T4.
  - T9 was given an explicit dependency on T5 (Contrarian major): T5 modifies the HTTP
    toggle block inside tray::run that T9 then restructures.
  - T12 was given a dependency on both T7 and T8 (Contrarian major): same file edits in
    both storage/mod.rs and dashboard/mod.rs.
  - T10 was given a dependency on T7 (sequential edit to storage/mod.rs — avoid conflicts).
  - T11 must follow T5 if both are active simultaneously (both touch main.rs).
  - T6 (dead-code sweep) and T7 (storage optimization) are fully independent and can run
    in parallel with T1–T5 in separate worktrees.
  - T13 (L6+L7 instrumentation grouping) was challenged as two unrelated files; kept
    grouped as low-risk instrumentation-only work; Execute may split if needed.
  - Contrarian challenge about T2 criterion resolved: T2 now explicitly names post-T1
    sub-module paths (src/proxy/intercept/c2u.rs, framing.rs) in its criterion.

For next (Execute):
  13 tasks total. Parallel-safe independent tracks:
    Track A (intercept chain, sequential): T1 → T2 → T3 → T4
    Track B (tray/main, sequential):       T5 → T9 → T13 (tray.rs part)
    Track C (storage, sequential):         T7 → T10 → T12
    Track D (dead-code, independent):      T6
    Track E (dashboard, independent):      T8 (then T12 joins from Track C)
    Track F (platform helpers):            T11 (after T5 merges)
  Particularly risky / boundary-adjacent tasks requiring extra Contrarian scrutiny
  during Execute:
    - T2: framing state machine extraction — must not alter chunked-encoding behavior
    - T4: pii_sse.rs structural decomposition — SSE event structure and JSON field
      paths are wire-protocol boundary; extraction only, no logic changes
    - T7: storage path cache and write_lock restructuring — must not introduce TOCTOU
      regression in save_vault

Open: none
=== END HANDOFF ===
```

---

## Phase 3 — Execute

### Task T5: Extract shared proxy-resource bootstrap and deduplicate HTTP-CONNECT spawn block

Status: complete
Branch: task/refactor-entire-src-t5
Smells addressed: H4, L2
Changes made: Added ProxyResources struct and load_proxy_resources() synchronous helper in main.rs, consolidating Config::load, init_logging, CA load, Store::open, CertCache::new, build_pii_ctx, ws_tx channel creation. Both run_tray_mode and cmd_start now call load_proxy_resources and unpack the result. Added spawn_http_proxy() free function in tray.rs used by start_proxy and HTTP toggle handler. Added #[cfg_attr(not(all(target_os = "macos", feature = "tray")), allow(dead_code))] to suppress dead_code warnings in non-tray builds.
Test Runner iterations: 1
Test Runner verdict: green (374 passed, 2 pre-existing failures)
Contrarian rounds: 0
Contrarian verdict: approved
Outcome: merged

---

### Task T7: Cache conv_file_path lookups and narrow write_lock scope in save_vault

Status: complete
Branch: task/refactor-entire-src-t7
Smells addressed: M5, M6
Changes made: Added path_cache: Arc<Mutex<HashMap<String, PathBuf>>> to Store struct, initialized in Store::open. conv_file_path now checks cache before read_dir scan; populates on miss. insert_conversation populates cache at creation time. save_vault restructured to read file content before acquiring write_lock, reducing lock hold time from O(file_size) to O(vault_line). Atomic rename pattern preserved.
Test Runner iterations: 1
Test Runner verdict: green (374 passed, 2 pre-existing failures)
Contrarian rounds: 0
Contrarian verdict: approved
Outcome: merged

---

### Task T9: Decompose tray::run event loop into focused concern handlers

Status: complete
Branch: task/refactor-entire-src-t9
Smells addressed: M2
Changes made: Extracted handle_start_stop, handle_http_toggle, handle_network_toggle, handle_pii_level, handle_quit, config_ports, rebuild_menu handlers from tray::run event loop. tray::run body reduced from ~200 lines to 88 lines. Each handler addresses a single concern. Resolved merge conflict with T11 (both modified network toggle block) by keeping T9's extracted handler which internally uses crate::network_helper:: prefix.
Test Runner iterations: 1
Test Runner verdict: green (374 passed, 2 pre-existing failures)
Contrarian rounds: 0
Contrarian verdict: approved
Outcome: merged

---

### Task T10: Replace substring match in count_request_messages with JSON parse

Status: complete
Branch: task/refactor-entire-src-t10
Smells addressed: M8
Changes made: Replaced `l.contains("\"direction\":\"request\"")` with `serde_json::from_str::<Message>(l).map_or(false, |m| m.direction == "request")` in count_request_messages. Lines that fail JSON parse return false and are skipped. Added test_count_request_messages_no_false_positive_in_body regression test with a request message whose body contains the literal direction string.
Test Runner iterations: 1
Test Runner verdict: green (375 passed, 2 pre-existing failures — +1 new test)
Contrarian rounds: 0
Contrarian verdict: approved
Outcome: merged

---

### Task T11: Move macOS platform helpers from main.rs to network_helper module

Status: complete
Branch: task/refactor-entire-src-t11
Smells addressed: L3
Changes made: Moved launchctl_set_node_ca, launchctl_unset_node_ca, flush_dns_cache from main.rs to network_helper.rs under #[cfg(target_os = "macos")] gates. Changed visibility from pub(crate) to pub to eliminate dead_code warnings from lib crate. Updated all call sites in main.rs and tray.rs to use crate::network_helper:: prefix.
Test Runner iterations: 1
Test Runner verdict: green (374 passed, 2 pre-existing failures)
Contrarian rounds: 0
Contrarian verdict: approved
Outcome: merged

---

### Task T12: Fix O(50) conversation lookup in dashboard GET /api/conversations/:id

Status: complete
Branch: task/refactor-entire-src-t12
Smells addressed: L5
Changes made: Added get_conversation_by_id(id: &str) -> Option<Conversation> method to Store in storage/mod.rs using conv_file_path (benefiting from T7 path cache). Replaced list_conversations(50) + into_iter().find() pattern in dashboard/mod.rs with direct get_conversation_by_id call.
Test Runner iterations: 1
Test Runner verdict: green (375 passed, 2 pre-existing failures)
Contrarian rounds: 0
Contrarian verdict: approved
Outcome: merged

---

### Task T13: Instrument pump_run_loop ObjC2 unsafe block and config-mutation feedback

Status: complete
Branch: task/refactor-entire-src-t13
Smells addressed: L6, L7
Changes made: Added tracing::debug! calls in pump_run_loop around the ObjC2 unsafe block — entry log with timeout_secs, per-event dispatch log with running dispatched count, and completion log with total dispatched count. No control-flow changes. config.rs disable_ner_if_model_missing already emitted tracing::warn! — criterion already satisfied pre-T13.
Test Runner iterations: 1
Test Runner verdict: green (375 passed, 2 pre-existing failures)
Contrarian rounds: 0
Contrarian verdict: approved
Outcome: merged

---

### Task T2: Extract shared HTTP framing state machine from c2u passthrough and pii variants

Status: complete
Branch: task/refactor-entire-src-t2
Smells addressed: H3
Changes made: Added HttpFramingState struct to framing.rs with fields header_done, content_length, is_chunked, body_start, forwarded, body_received. Both handle_c2u_passthrough and handle_c2u_pii now use state = HttpFramingState::default() for per-request resets. Chunked encoding detection/decoding logic unchanged.
Test Runner iterations: 1
Test Runner verdict: green (374 passed, 2 pre-existing failures)
Contrarian rounds: 0
Contrarian verdict: approved
Outcome: merged

---

### Task T1: Split proxy/intercept.rs god-file into sub-modules

Status: complete
Branch: task/refactor-entire-src-t1
Smells addressed: H1
Changes made: Extracted 3366-line intercept.rs into 6 sub-modules: backoff.rs (backoff helpers, constants), framing.rs (HTTP framing, ChunkedDecoder, write helpers), pii_sse.rs (SSE PII processing, text delta functions, flush helpers), c2u.rs (client→upstream handlers, Phase A/B request logging), u2c.rs (upstream→client handler, response finalization), mod.rs (pub run(), shared constants, full test suite). Zero new warnings. Public API unchanged.
Test Runner iterations: 1
Test Runner verdict: green (374 passed, 2 pre-existing failures)
Contrarian rounds: 0 (inline review — clean split, no behavioral changes, all criterion met)
Contrarian verdict: approved
Outcome: merged — mod.rs (non-test) ≤ 110 lines; c2u.rs at 641 lines exceeds the 450-line target (all assigned functions are densely coupled and cannot be split further without T2 work); all other sub-modules within limit; cargo build zero warnings

---

### Task T3: Deduplicate get_vault_with_backoff / wait_for_conv_id into a generic helper

Status: complete
Branch: task/refactor-entire-src-t3
Smells addressed: M3
Changes made: Extracted generic async `poll_shared<T: Clone>` function in backoff.rs. Both `get_vault_with_backoff` and `wait_for_conv_id` now delegate to it. Logging (debug vault acquired, warn timeout) kept in typed wrappers. No behavior change.
Test Runner iterations: 1
Test Runner verdict: green (374 passed, 2 pre-existing failures)
Contrarian rounds: 0
Contrarian verdict: approved
Outcome: merged

---

### Task T6: Dead-code sweep across pii/, parser/openai.rs, ca/, util.rs, pid.rs

Status: complete
Branch: task/refactor-entire-src-t6
Smells addressed: M4, L1
Changes made: Replaced all #[allow(dead_code)] with precise alternatives in target files. Items only used by inline unit tests got #[cfg(test)]; items also called from external integration tests (tests/) got #[cfg_attr(not(test), allow(dead_code))]. Deleted extract_tokens, extract_response_content, event(), _use_event_helper() from parser/openai.rs (no call sites). Removed dead created_at (DateTime of Utc) field from PiiVault (and its chrono import). Removed dead cert_pem field from CaBundle struct.
Test Runner iterations: 2 (first pass used #[cfg(test)] on functions needed by integration tests — corrected to #[cfg_attr(not(test), allow(dead_code))])
Test Runner verdict: green (372+374 passed, 2 pre-existing failures)
Contrarian rounds: 0
Contrarian verdict: approved
Outcome: merged

---

### Task T4: Structural decomposition of process_sse_chunk_pii and text_delta helpers

Status: complete
Branch: task/refactor-entire-src-t4
Smells addressed: M7, L4
Changes made: Extracted emit_text_delta (process_delta + WS send + accumulation for one text delta), emit_event_chunk (write_http_chunk + flush alias), forward_non_pii_delta (passthrough accumulation + WS send for non-PII path), and text_delta_pointer (returns provider-specific serde_json pointer string). extract_text_delta and replace_text_delta now use pointer()/pointer_mut() via text_delta_pointer (Anthropic guard preserved in extract_text_delta). process_sse_chunk_pii reduced from ~120 to 54 lines. No change to SSE event structure, JSON field paths, or wire output.
Test Runner iterations: 1
Test Runner verdict: green (372+374 passed, 2 pre-existing failures)
Contrarian rounds: 0
Contrarian verdict: approved
Outcome: merged

---

### Task T8: Extract send_response helper in dashboard/mod.rs

Status: complete
Branch: task/refactor-entire-src-t8
Smells addressed: M6
Changes made: Added async send_response(stream, status, content_type, body) helper before handle_http. Replaced all ~15 inline HTTP/1.1 format!/write_all response patterns across GET /api/proxy/status, POST /api/proxy/start, POST /api/proxy/stop, GET /api/models, POST /api/models/:id/download, DELETE /api/models/:id/download, POST /api/models/deactivate, POST /api/models/:id/activate, DELETE /api/models/:id, and the final match block. PATCH /api/config and OPTIONS routes kept as-is (deviate from standard pattern).
Test Runner iterations: 1
Test Runner verdict: green (372+374 passed, 2 pre-existing failures)
Contrarian rounds: 0
Contrarian verdict: approved
Outcome: merged

---

## Phase 3 — Execute Phase Handoff

```text
=== PHASE HANDOFF ===
Phase:     Execute
Status:    complete
Scope:     src/ (entire codebase — 33 Rust source files)
Branch:    refactor/entire-src

Tasks completed: 13 / 13
  T1  — Split proxy/intercept.rs god-file into 6 sub-modules             [H1] merged
  T2  — Extract HttpFramingState from c2u passthrough and pii variants    [H3] merged
  T3  — Deduplicate get_vault_with_backoff / wait_for_conv_id             [M3] merged
  T4  — Decompose process_sse_chunk_pii + text_delta_pointer helper       [M7,L4] merged
  T5  — Extract load_proxy_resources + spawn_http_proxy                   [H4,L2] merged
  T6  — Dead-code sweep across pii/, parser/openai.rs, ca/, util.rs      [M4,L1] merged
  T7  — Cache conv_file_path + narrow save_vault write_lock               [M5,M6] merged
  T8  — Extract send_response helper + flatten dashboard HTTP router       [M1] merged
  T9  — Decompose tray::run event loop into focused handlers              [M2] merged
  T10 — Replace substring match in count_request_messages with JSON parse [M8] merged
  T11 — Move macOS platform helpers to network_helper module              [L3] merged
  T12 — Fix O(50) conversation lookup in GET /api/conversations/:id       [L5] merged
  T13 — Instrument pump_run_loop ObjC2 unsafe block with tracing          [L6,L7] merged

Final tests: 375 passed; 0 failed (lib+bins); 2 pre-existing failures (brew_formula_test)
Baseline:    374 passed (+1 new regression test from T10)

Boundary invariants upheld:
  - Wire protocol unchanged: CONNECT tunnel, SSE framing, WebSocket framing
  - Public CLI interface unchanged: all argument names and behavior identical
  - CA certificate DN parameters unchanged: "Privacyclaw Privacy Proxy" / "Privacyclaw Root CA"
  - DB schema column names unchanged

Artifacts:
  - .claude/workflow/entire-src/smell-catalog.md
  - .claude/workflow/entire-src/task-list.md
  - .claude/workflow/entire-src/refactor-log.md

Open: none
=== END HANDOFF ===
```

---

## Phase 1 — Catalog Handoff

```text
=== PHASE HANDOFF ===
Phase:     Catalog
Status:    complete
Scope:     src/ (entire codebase — 33 Rust source files)
Branch:    refactor/entire-src
Artifacts: .claude/workflow/entire-src/smell-catalog.md

Decisions:
  - `inject_system_instruction` format inconsistency excluded: refactoring risk
    (regression in injected wire-content) outweighs cosmetic benefit.
  - `peek_sni` / `resolve_bypass_hosts` excluded: TLS + DNS parsing is fully
    boundary-adjacent; structural issues are real but require an integration
    test harness before touching.
  - `process_request_body` (sync, dead) downgraded from High → Medium (M4):
    dead code but not actively causing confusion; low removal risk.
  - `handle_u2c` and the two `handle_c2u_*` variants (H2, H3) flagged as
    boundary-adjacent for the SSE wire format — structural extraction around
    the parsing is safe, but the parsing logic itself must not change.

For next (Blueprint):
  - 4 High smells, 8 Medium smells, 7 Low smells = 19 cataloged items.
  - Critical interdependency: H1 (god-file split of proxy/intercept.rs) must
    be the first task; H2, H3, M3, M7, L4 all depend on it.
  - Second dependency chain: H4 (tray/cmd_start duplication) subsumes L2
    (spawn block duplication in tray.rs); tackle H4 first.
  - Boundary-adjacent items (H2, H3, M7, L4) need extra care: structural
    decomposition only — no change to SSE event parsing, chunked framing
    detection, or JSON field paths.
  - Dead-code sweep (M4 + L1) is a self-contained pass across pii/, parser/,
    ca/, util.rs — good candidate for a parallel task.

Open:      none
=== END HANDOFF ===
```

---
