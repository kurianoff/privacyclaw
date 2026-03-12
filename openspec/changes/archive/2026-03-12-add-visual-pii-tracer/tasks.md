# Tasks: add-visual-pii-tracer

## 1. Storage model changes

- [x] 1.1 Add `pii_processed: Option<bool>` to `Message` struct in `storage/mod.rs` with `#[serde(default, skip_serializing_if = "Option::is_none")]`
- [x] 1.2 Add `confidence: Option<f32>` to `StoredVaultRecord` with same serde attributes
- [x] 1.3 Add `MessageDetection` struct: `{ message_id: String, entity_type: String, original_masked: String, synthetic: String, tier: u8, confidence: f32 }`
- [x] 1.4 Implement `Store::insert_detections(conv_id: &str, detections: &[MessageDetection]) -> Result<()>` — appends `{"type":"detection", ...}` lines to NDJSON file
- [x] 1.5 Implement `Store::load_detections(conv_id: &str, message_id: Option<&str>) -> Result<Vec<MessageDetection>>` — scans file for `"type":"detection"` lines, filters by message_id when present
- [x] 1.6 Add unit tests: round-trip `insert_detections` + `load_detections` (with and without message_id filter), empty file returns empty vec

## 2. Vault confidence

- [x] 2.1 Add `confidences: Vec<f32>` parallel vec to `PiiVault` in `pii/vault.rs`
- [x] 2.2 Update `PiiVault::add_mapping` signature: add `confidence: f32` parameter; push to `confidences` vec
- [x] 2.3 Update `PiiVault::insert_mapping_raw`: same change
- [x] 2.4 Update `PiiVault::new`: initialise `confidences: vec![]`
- [x] 2.5 Replace `quads()` with `quints()` returning `(&str, &str, &str, u8, f32)` — or add `quints()` alongside `quads()` if existing callers are too numerous to migrate in one pass
- [x] 2.6 Update `from_records`: pass `record.confidence.unwrap_or(0.0)` to `insert_mapping_raw`
- [x] 2.7 Update `save_vault` call in `finalize_response` (intercept.rs) to pass confidence per tuple
- [x] 2.8 Update `Store::save_vault` signature to accept confidence; write it to `StoredVaultRecord`
- [x] 2.9 Update all `add_mapping` call sites in `pii/mod.rs` and `pii/tier3.rs` to pass confidence
- [x] 2.10 Add unit tests: confidence round-trips through vault save/load; legacy `None` confidence loads as `0.0`

## 3. `log_request` split (critical — enables content_masked on requests)

- [x] 3.1 Extract Phase A from `log_request` into `create_or_find_conversation(provider, fingerprint, model, host, store, ws_tx) -> Result<String>` — returns `conv_id`. Broadcasts `ConversationStart`. No message storage.
- [x] 3.2 Rename / refactor remaining `log_request` logic into `store_request_messages(original_body, replaced_body: Option<&[u8]>, pii_processed: bool, conv_id: &str, provider, store, ws_tx) -> Result<Vec<String>>` — returns stored message IDs. Broadcasts `Message` WS events including `content_masked`.
- [x] 3.3 Update `handle_c2u_pii` call order:
  - Call Phase A before pipeline → get `conv_id`
  - Run PII pipeline
  - Call Phase B after pipeline → pass `replaced_body`, `pii_processed=true`
- [x] 3.4 Update `handle_c2u_passthrough`: call Phase A, then Phase B with `replaced_body=None, pii_processed=false`
- [x] 3.5 Populate `PiiDetection.message_id` from Phase B's returned message IDs (last ID for the request batch)
- [x] 3.6 Call `Store::insert_detections` after PII pipeline with the populated `Vec<PiiDetection>`
- [x] 3.7 Add `message_id: Option<String>` field to `PiiDetection` struct in `pii/mod.rs`
- [x] 3.8 Add unit tests: Phase A returns same conv_id for same fingerprint; Phase B stores `content_masked`; detection records written with correct message_id

## 4. WsEvent extension

- [x] 4.1 Add `content_masked: Option<String>` and `pii_processed: Option<bool>` to `WsEvent::Message` variant in `dashboard/mod.rs`
- [x] 4.2 Populate both fields in Phase B's WS broadcast (from `store_request_messages`)
- [x] 4.3 Add unit test: WS event JSON includes `content_masked` when set; existing clients ignoring unknown fields are unaffected

## 5. Dashboard API

- [x] 5.1 Add `handle_detections_api` in `dashboard/mod.rs`: parse `conv_id` and optional `message_id` query param, call `store.load_detections`, return JSON array
- [x] 5.2 Add routing for `GET /api/conversations/:id/detections` before the vault check
- [x] 5.3 Update `handle_vault_api`: add `"confidence": r.confidence` to each JSON object
- [x] 5.4 Update `list_conversations` in `storage/mod.rs`: replace `entries.truncate(10)` with `entries.truncate(limit)` where `limit` defaults to 50 and is capped at 200
- [x] 5.5 Update `GET /api/conversations` handler to read optional `?limit=N` query param and pass to storage
- [x] 5.6 Add unit tests: detections endpoint returns filtered records; vault endpoint includes confidence; conversations endpoint respects limit param

## 6. Frontend: compare view column 2 fix

- [x] 6.1 In `renderCompareView` (app.js), update column 2 logic: use `msg.content_masked` when present; fall back to `applyPiiMasking` when `msg.pii_processed == null`; show `(approx)` badge for fallback case; show clean column 1 copy when `pii_processed == false`
- [x] 6.2 Add `buildApproxBadge()` helper returning `<span class="approx-badge" title="...">approx</span>`
- [x] 6.3 Fix pre-existing dedup bug: standardise vault dedup key to `type|original_masked|synthetic` in both `handlePiiDetected` (line ~266) and `loadVault` (line ~148)

## 7. Frontend: span highlighting

- [x] 7.1 Extend `buildHighlightedHtml(text, terms, cssClass, tooltipFn)` to accept class and tooltip factory arguments (backward compatible: existing calls with 2 args keep current behaviour)
- [x] 7.2 Update `buildCompareBubble` to pass appropriate class and tooltip for each column
- [x] 7.3 Add CSS: `.pii-orig { border-bottom: 2px dashed #f59e0b; background: #fff3cd; }`, `.pii-synth { border-bottom: 2px solid #3b82f6; background: #cfe2ff; }`, `.pii-restored { border-bottom: 2px solid #22c55e; background: #d1e7dd; }`
- [x] 7.4 Add `.tier-badge` superscript style
- [x] 7.5 Add `.approx-badge` amber pill style

## 8. Frontend: turn navigator

- [x] 8.1 Implement `buildTurns(messages)` — groups messages into `{index, requests[], responses[]}` pairs; skips system messages in turn numbering
- [x] 8.2 Implement `buildTurnNav(turns, vault)` — renders `<div class="turn-nav">` with numbered chips; adds red `.turn-badge` when detection count > 0
- [x] 8.3 Wire chip click to `scrollToTurn(index)` — `element.scrollIntoView({behavior:'smooth'})`; add `data-turn` attribute to first message row of each turn
- [x] 8.4 Insert turn nav above compare view scroll area in `renderCompareView`
- [x] 8.5 In live mode: append new chip on each `WsEvent::Message` with `direction=request`; increment badge on each `WsEvent::PiiDetected`
- [x] 8.6 Add `.turn-nav`, `.turn-chip`, `.turn-badge` CSS

## 9. Frontend: detection sidebar

- [x] 9.1 Add `<div id="detection-sidebar" class="hidden">` to `index.html`
- [x] 9.2 Implement `renderDetectionSidebar(convId, messageId)` — fetches `/api/conversations/:id/detections?message_id=:msgId`, renders entity table with confidence `<progress>` bars; handles empty response and legacy "no attribution" case
- [x] 9.3 Attach click handler to message bubbles in `buildCompareBubble` — calls `renderDetectionSidebar`; clicking elsewhere closes sidebar
- [x] 9.4 Add `#detection-sidebar` CSS: right-side panel, width 320px, box-shadow, z-index above compare columns

## 10. Frontend: conversation summary bar

- [x] 10.1 Implement `buildConvSummary(vault)` — computes totals from in-memory vault array; returns `<div class="conv-summary">` DOM element
- [x] 10.2 Insert summary bar at top of compare view container in `renderCompareView`
- [x] 10.3 Update summary on each `WsEvent::PiiDetected` during live session
- [x] 10.4 Add `.conv-summary` CSS

## 11. Tests

- [x] 11.1 Unit: `Message` round-trips with new fields; old JSON without new fields deserializes with `None`
- [x] 11.2 Unit: `StoredVaultRecord` with and without `confidence`; backward compat
- [x] 11.3 Unit: detection log insert + load (filtered and unfiltered)
- [x] 11.4 Unit: `PiiVault` confidence stored in parallel vec; `quints()` returns correct tuples; `from_records` handles `None` confidence
- [x] 11.5 Unit: Phase A / Phase B split — `conv_id` returned by Phase A is used by Phase B; `content_masked` populated correctly for multi-message batch
- [x] 11.6 Unit: `WsEvent::Message` serializes with optional fields; extra fields ignored by old JSON consumers
- [x] 11.7 Unit: detections API endpoint returns filtered JSON; vault API includes confidence
- [x] 11.8 Integration: outbound request with PII → `content_masked` stored → compare view col 2 matches stored value → detection record written with correct message_id
- [x] 11.9 Run `cargo test` — all tests pass
- [x] 11.10 Run `cargo clippy -- -D warnings` — clean
