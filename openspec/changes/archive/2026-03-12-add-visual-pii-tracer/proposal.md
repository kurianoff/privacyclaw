# Change: Add Visual PII Tracer

## Why

The dashboard already has a 4-column compare view (Original / Sent to LLM / LLM Response / Delivered), but column 2 ("Sent to LLM") is reconstructed client-side using a naive string replacement that diverges from the server-side Aho-Corasick pipeline — especially for T3 standalone tokens, multipart content arrays, and overlapping PII spans. The vault API omits confidence scores for historical sessions. There is no way to see which conversation turn triggered a specific detection, and the UI has a pre-existing deduplication bug where live WS events and vault-load events use different dedup keys.

This change makes the visual tracer complete and reliable:
- Server stores the actual replaced request body (not a client-side approximation)
- Vault records carry confidence scores, persisted across restarts
- A lightweight per-message detection log enables per-turn attribution in the sidebar
- Span highlighting uses per-column colour treatment (amber / blue / green)
- A turn navigator chip bar lets the user jump to any turn with detection badges
- A per-message detection sidebar shows entity details on bubble click
- A conversation summary bar shows total PII by tier and type

## What Changes

### `src/storage/mod.rs`
- `Message`: add `pii_processed: Option<bool>` — distinguishes "pipeline ran, no PII found" (`Some(false)`) from "legacy message, pipeline never ran" (`None`)
- `Message.content_masked` on **request** messages: populated with the PII-replaced text content (currently always `None` for requests)
- `StoredVaultRecord`: add `confidence: Option<f32>` (persists confidence across restarts)

### `src/pii/vault.rs`
- `PiiVault`: add `confidences: Vec<f32>` parallel vec alongside existing `tiers`, `pii_types`, etc.
- `PiiVault::add_mapping`: add `confidence: f32` parameter
- `PiiVault::insert_mapping_raw`: add `confidence: f32` parameter
- Extend `quads()` / `to_records()` to include confidence

### `src/pii/mod.rs`
- `PiiDetection`: add `message_id: Option<String>` — set after `log_request` returns stored message IDs

### New: per-message detection log
- New struct `MessageDetection` in storage: `{ message_id, entity_type, original_masked, synthetic, tier, confidence }`
- New `Store::insert_detections(conv_id, detections: &[MessageDetection])` — appends detection records to the NDJSON file as `"type":"detection"` lines
- New `Store::load_detections(conv_id, message_id: Option<&str>)` — loads detection lines, optionally filtered by message_id

### `src/proxy/intercept.rs`
- Split `log_request` into two phases:
  - Phase A (before pipeline): conversation creation, `conv_id` assignment, `ConversationStart` WS broadcast
  - Phase B (after pipeline): message storage with `content_masked`, `Message` WS broadcast
- Phase B receives `replaced_body: Option<&[u8]>` and `pii_processed: bool`
- `handle_c2u_pii`: capture returned message IDs from Phase B; set on `PiiDetection` before WS broadcast and before `insert_detections`
- `finalize_response`: update `save_vault` call to include confidence per record

### `src/dashboard/mod.rs`
- `WsEvent::Message`: add `content_masked: Option<String>` and `pii_processed: Option<bool>`
- `handle_vault_api`: add `"confidence"` to JSON response
- New endpoint `GET /api/conversations/:id/detections?message_id=...` → filtered `MessageDetection` records
- New endpoint (or extend existing): `/api/conversations` supports `?limit=N` (default 50, was hardcoded 10)

### `src/dashboard/assets/app.js`
- `renderCompareView` column 2: use `msg.content_masked` when present; fall back to `applyPiiMasking` with amber `(approx)` badge when `msg.pii_processed` is absent (legacy) or `content_masked` is null
- `buildHighlightedHtml`: extend to accept highlight CSS class and tooltip factory
- Per-column highlight treatment: amber (`pii-orig`) / blue (`pii-synth`) / green (`pii-restored`)
- Add `buildTurnNav(messages, vault)` — numbered chips, red detection-count badges
- Add `renderDetectionSidebar(convId, messageId)` — fetches `/detections`, renders entity table with confidence bars
- Add `buildConvSummary(vault)` — summary bar (total, by tier, by type)
- Fix pre-existing dedup bug: standardise dedup key to `type|original|synthetic` in both `handlePiiDetected` and `loadVault`

### `src/dashboard/assets/style.css`
- Add `.pii-orig`, `.pii-synth`, `.pii-restored` highlight styles
- Add `.tier-badge`, `.approx-badge`, `.turn-nav`, `.turn-chip`, `.turn-badge`, `#detection-sidebar`

## Backward Compatibility

All new fields use `#[serde(default, skip_serializing_if = "Option::is_none")]`. Existing NDJSON files without the new fields deserialise correctly:
- `pii_processed: None` → shown as legacy in UI ("approx" badge)
- `confidence: None` → shown as dash in sidebar confidence column
- Missing `"type":"detection"` lines → sidebar shows "no per-message attribution available"

## Out of Scope

- Byte-level span offset storage (JS substring matching sufficient)
- Per-detection timestamp (turn timestamp via `Message.timestamp` is sufficient)
- Cross-conversation PII search
- Export / download of compare view
- Vault diff view between turns
- Re-anonymization replay of historical messages
- WebSocket event replay for late-joining clients
