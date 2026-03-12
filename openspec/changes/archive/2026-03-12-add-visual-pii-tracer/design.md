# Design: Visual PII Tracer

## Data Flow

### Outbound (existing gaps fixed)

```
Client request
  │
  ▼
Phase A — log_request (before pipeline)
  │  Creates conversation, assigns conv_id, broadcasts ConversationStart WS event
  │  Returns: stored_msg_ids (one per parsed message in request body)
  │
  ▼
PII pipeline — process_request_body_async
  │  Detects PII, replaces with synthetics, returns (forward_body, Vec<PiiDetection>)
  │  Each PiiDetection gets message_id from stored_msg_ids[last_index]
  │
  ▼
Phase B — store_request_messages (after pipeline)
  │  Stores Message records with content_masked = extracted text from forward_body
  │  Sets pii_processed = true (pipeline ran)
  │  Broadcasts Message WS event with content_masked
  │
  ▼
Store::insert_detections  ←  Vec<PiiDetection> with message_id
  │  Appends "type":"detection" lines to NDJSON file
  │
  ▼
upstream_write → LLM
```

### Inbound (no changes — already correct)

```
LLM SSE stream
  │
  ▼
ReplacementBuffer — accumulates synthetic_accumulated + restored accumulated
  │
  ▼
finalize_response
  │  Stores Message with content = restored, content_masked = synthetic_accumulated
  │  save_vault with (original, synthetic, pii_type, tier, confidence) tuples
```

## Critical Design Decision: Split `log_request`

The existing `log_request` creates the conversation (and therefore `conv_id`) as a side effect. The PII pipeline needs `conv_id` to look up the vault. This creates a dependency:

```
conv_id needed by pipeline → created by log_request → must run before pipeline
content_masked needed for storage → comes from pipeline → must run after pipeline
```

Resolution: split into two phases called from `handle_c2u_pii`:

**Phase A** (`create_or_find_conversation`): idempotent conversation creation using `find_conversation_by_fingerprint` / `insert_conversation`. Returns `conv_id`. Broadcasts `ConversationStart`. **No message storage.**

**Phase B** (`store_request_messages`): inserts `Message` records with `content_masked` populated from the replaced body. Broadcasts `Message` WS events. Returns `Vec<String>` of stored message IDs.

On pipeline failure/timeout: Phase B is called with `replaced_body = None` and `pii_processed = Some(false)`. Messages are always stored; `content_masked` is `None` for that batch.

## Per-Message Detection Log (NOT vault metadata)

The vault's `add_mapping` is idempotent: if "alice@acme.com" appears in turn 1 and turn 5, only turn 1's record is kept. Embedding `message_id` in vault records would give turn 5's sidebar zero detections for that entity.

Resolution: separate detection log. Each time the PII pipeline produces a `PiiDetection`, regardless of whether the vault entry is new or reused, a `MessageDetection` record is written:

```json
{"type":"detection","message_id":"uuid","entity_type":"EMAIL","original_masked":"[EMAIL]","synthetic":"alice.brown@example.com","tier":1,"confidence":1.0}
```

The vault retains only `(original, synthetic, pii_type, tier, confidence)` — no message-id. The detection log is append-only and never deduplicated.

`Store::load_detections(conv_id, message_id)` scans the NDJSON file for `"type":"detection"` lines and filters by `message_id`. For conversations with 200 turns and ~5 detections/turn, this is ~1000 lines — well within the O(n) scan budget.

## `content_masked` for Multi-Turn Request Bodies

`log_request` (Phase B) parses both the original body and the replaced body to produce pairs:

```rust
let orig_messages = parser::parse_request(provider, &original_body)?;
let repl_messages = replaced_body
    .and_then(|b| parser::parse_request(provider, b));
```

For each `parser::Message` at index `i`:
- `content_masked = repl_messages.as_ref().and_then(|m| m.get(i)).map(|m| m.content.clone())`
- `pii_processed = Some(replaced_body.is_some())`

If the replaced body fails to parse (malformed synthetic value, etc.): all messages in the batch get `content_masked = None`, `pii_processed = Some(true)` (pipeline ran, storage failed). Log WARN.

## Column 2 Fallback Strategy

| `pii_processed` | `content_masked` | What column 2 shows |
|---|---|---|
| `None` (legacy) | `None` | Client-side `applyPiiMasking` + amber `(approx)` badge |
| `Some(false)` | `None` | Original text (same as col 1) — no PII was detected |
| `Some(true)` | `Some(text)` | Stored replaced text — accurate |
| `Some(true)` | `None` | Client-side fallback + amber `(approx)` badge (parse failure) |

## JS Dedup Fix

Current bug: `handlePiiDetected` (WS live) deduplicates on `type|original|synthetic`, while `loadVault` (HTTP history) deduplicates on `type|original_masked|synthetic`. When both paths fire for the same entity, the vault table shows a duplicate row.

Fix: standardise all dedup to `type|original_masked|synthetic`. `original_masked` is what the API returns (e.g. `[EMAIL]`) — always present in both paths.

## Vault Confidence

`PiiVault` parallel vecs — current: `original_values`, `synthetic_keys`, `pii_types`, `tiers`. Add: `confidences: Vec<f32>`.

`add_mapping(original, synthetic, pii_type, tier, confidence)` pushes to all five vecs atomically. `insert_mapping_raw` gains the same parameter. `from_records` passes `record.confidence.unwrap_or(0.0)` (sentinel: 0.0 means legacy record where confidence was not stored).

The UI renders confidence 0.0 as `—` (dash) rather than a `0%` bar to avoid misleading users about legacy records.

## Turn Navigator Algorithm

```javascript
function buildTurns(messages) {
    const turns = [];
    let current = null;
    for (const msg of messages) {
        // system messages belong to no turn; skip for chip numbering
        if (msg.role === 'system') continue;
        if (msg.direction === 'request') {
            current = { index: turns.length, requests: [msg], responses: [] };
            turns.push(current);
        } else if (current) {
            current.responses.push(msg);
        }
    }
    return turns;
}
```

Detection count per turn: filter `vault` entries where `message_id` matches any request in the turn. In live mode, count is updated on each `WsEvent::PiiDetected`.

## Streaming Live Mode Behaviour

| Column | Live state | Final state |
|---|---|---|
| Col 1 (Original) | Populates on `WsEvent::Message` (direction=request) | Unchanged |
| Col 2 (Sent to LLM) | Populates on same `WsEvent::Message` using `msg.content_masked` | Unchanged |
| Col 3 (LLM Response) | Accumulates on `WsEvent::TextDelta` deltas in real-time | Unchanged |
| Col 4 (Delivered) | Spinner until `WsEvent::ResponseComplete`, then final restored text | Loaded from storage |

Turn chips append on each new `WsEvent::Message` with `direction=request`. Badge count increments on each `WsEvent::PiiDetected` matched to the current turn.
