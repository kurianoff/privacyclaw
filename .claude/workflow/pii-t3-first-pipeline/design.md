# Design Document: Adaptive PII Protection — T3-First Pipeline (Part I)

**Feature**: Implement Part I of the Adaptive PII Protection: T3-First Pipeline
**Branch**: feature/pii-t3-first-pipeline
**Status**: Draft v0.4 — Architect pass (post-contrarian round 2)
**Slug**: pii-t3-first-pipeline

---

## 0. Executive Summary

Part I restructures privacyclaw's outbound PII detection pipeline so the SLM (Tier 3) operates as a *first pass* on the raw user message before Tier 1 (regex) and Tier 2 (GLiNER NER) run. All tiers now emit a unified XML token format. The inbound `ReplacementBuffer` gains a 5-level cascade matcher. A system instruction is injected to promote LLM verbatim reproduction of tokens.

Scope boundary: Part II (Surface Form Oracle, hypothesis generation, training data collection, MVM) is explicitly deferred.

---

## 1. Background

### 1.1 Current Architecture

```
Raw message → T1 regex → T2 GLiNER → merged spans → T3 /disambiguate → replace → vault
```

Synthetics today are format-preserving bare values (e.g. `alice.smith@example.com`, `10.142.7.3`). The inbound buffer uses Aho-Corasick over these bare synthetics.

The current T3 endpoint is `/v1/chat/completions` (standard llama-server API). The current T3 standalone path (`slm_standalone: true`) calls `detect_and_rewrite`, which uses `§value§` delimiters and a fragile text-diffing alignment algorithm (`extract_token_pairs`).

### 1.2 Problems Solved

**A — Synthetic contamination**: T3 currently sees text already containing format-preserving synthetics, which are indistinguishable from real PII. This can cause double-replacement and vault corruption.

**B — Coverage gap**: T3's contextual understanding is wasted on pre-processed text. Pattern-free secrets (safe codes, partial card fragments) require unmodified context.

**C — Delimiter fragility**: The `§value§` format is not in LLM training data. XML tags are, making verbatim reproduction substantially more reliable.

---

## 2. Token Format Change

### 2.1 Unified XML Token Format

All synthetic replacements, regardless of detecting tier, use:

```
<pii id="TOKEN_ID">DISPLAY_VALUE</pii>
```

- **TOKEN_ID**: 8-character base62 string derived from `HMAC-SHA256(key=conversation_id, data=entity_index_as_string)[0..6_bytes]` encoded as base62. Entity index is the vault's current mapping count at insertion time (same seeding strategy as existing `rng_seed` in `PiiVault`).
- **DISPLAY_VALUE**: format-preserving synthetic value from `synth.rs` (unchanged generator). Never empty.

**Base62 alphabet**: `0-9A-Za-z` (62 chars). 6 bytes → 8 base62 chars. Collision space: 62^8 ≈ 218 trillion. Sufficient within a conversation (vault TTL 24h; realistic max entity count per conversation ~thousands).

### 2.2 Token ID Generation

`sha2` is already in `Cargo.toml`. `hmac` is NOT present. Rather than add a new crate, token IDs are generated using `sha2::Sha256` directly: SHA-256 of the concatenation `conversation_id + ":" + entity_index_decimal` gives 32 bytes; the first 6 bytes are base62-encoded to produce an 8-character TOKEN_ID.

```rust
// New function: pii/vault.rs
fn generate_token_id(conversation_id: &str, entity_index: u64) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(conversation_id.as_bytes());
    h.update(b":");
    h.update(entity_index.to_string().as_bytes());
    let result = h.finalize();
    base62_encode(&result[..6])
}
```

This is not HMAC (no key separation), but it is sufficient for non-cryptographic, per-conversation token uniqueness. Collision risk within a conversation: negligible at 62^8 ≈ 218 trillion with realistic entity counts in the thousands.

**No new crates required.** `sha2` is already present; `sha1` (used in vault RNG seeding) remains unchanged.

### 2.3 Format Emission

The replacement string written into the JSON body is:
```
<pii id="a3f9b2c1">Maria Blinke</pii>
```

This replaces the current bare synthetic (`Maria Blinke` written directly).

---

## 3. Tier Activation Rules

### 3.1 Config Validation

`PiiTiersConfig` has three booleans: `regex`, `ner`, `slm`. The following combinations are **INVALID** and must return a config-load error:

| Config | Status |
|---|---|
| `ner=true, regex=false, slm=false` | INVALID — T2 alone |
| `ner=true, regex=false, slm=true` | INVALID — T2+T3 without T1 |

**Codebase finding**: The existing `validate_pii_tiers` in `config.rs` (line 408) currently rejects `{slm:true, regex:true, ner:false}` with the error "Tier 3 depends on Tier 1 + Tier 2". This must be relaxed — T3+T1 (no T2) is a valid combination per the new spec. The validation must be rewritten to:

```rust
fn validate_pii_tiers(tiers: &PiiTiersConfig) -> anyhow::Result<()> {
    if tiers.ner && !tiers.regex {
        anyhow::bail!("pii.tiers.ner requires pii.tiers.regex = true");
    }
    // T3 alone (standalone) is valid; T3+T1 is valid; T3+T1+T2 is valid.
    // T2+T3 without T1 is already caught by the ner-without-regex rule above.
    Ok(())
}
```

The `is_t3_standalone` helper in `config.rs` and its tests remain valid and unchanged.

### 3.2 Pipeline Activation Matrix

| T1 | T2 | T3 | Behaviour |
|---|---|---|---|
| ✓ | | | T1 regex → replace. No T3. |
| ✓ | ✓ | | T1+T2 merged spans → replace. No T3. |
| | | ✓ | **T3 first pass** → replace. No T1/T2 follow-up. No T3 disambiguation. |
| ✓ | | ✓ | T3 first pass; then T1 on T3-modified text with exclusion zones. |
| ✓ | ✓ | ✓ | T3 first pass; then T1+T2 on T3-modified text with exclusion zones; then T3 /disambiguate for low-confidence spans (confidence < 0.7). |

### 3.3 slm_standalone Removal

The `slm_standalone: bool` field in `PiiPipeline` is **removed**. Its behaviour is now fully expressed by `tiers.slm=true, tiers.regex=false, tiers.ner=false` (the T3-only row above).

**Codebase impact**: `slm_standalone` is referenced in two places beyond `pii/mod.rs`:

1. `proxy/intercept/c2u.rs` line 324: `p.pipeline.slm_standalone && p.mode == PiiMode::Replace` — this gates system instruction injection. Under the new design, the gate changes to: `p.mode == PiiMode::Replace` (inject whenever PII replacement mode is active, regardless of which tiers are enabled). This broadens injection correctly — any tier combination in Replace mode should inject the system reminder.

2. Test functions `pipeline_slm_standalone_flag_true_and_tier2_none` and `pipeline_full_stack_slm_standalone_false` in `pii/mod.rs` — these must be deleted or rewritten against the new tier-based control flow.

---

## 4. New T3 SLM Endpoint: /replace

### 4.1 API Contract

```
POST /replace
Content-Type: application/json

Body: {
  "text": "...",
  "conversation_id": "..."
}

Response: {
  "modified_text": "...",
  "replacements": [
    {
      "original": "Anne Nicole",
      "display_value": "Maria Blinke",
      "entity_type": "person_name",
      "token_id": "a3f9b2c1",
      "start": 11,
      "end": 22
    }
  ]
}
```

**Critical**: The SLM returns explicit `replacements` with `original`, `start`, `end`. No diffing of `text` vs `modified_text`. The proxy does not attempt inference — it trusts the SLM's span report.

Note: `start`/`end` are byte offsets into the original `text` (not `modified_text`).

### 4.2 Failure Mode

If the `/replace` endpoint is unavailable (timeout, non-200, JSON parse error): log WARN, skip Stage 1 (treat as if T3 produced no replacements), proceed to Stage 2 with the original text. **Fail-open** for availability; fail-safe for privacy is already provided by T1/T2 running on the original text in this case.

This is consistent with the existing `disambiguate` failure behavior.

### 4.3 Client Implementation

New method on `SlmSidecar`:

```rust
pub async fn replace(&self, text: &str, conversation_id: &str) -> Option<ReplaceResponse>
```

Where `ReplaceResponse` is a new struct:

```rust
#[derive(Deserialize)]
pub struct ReplaceResponse {
    pub modified_text: String,
    pub replacements: Vec<ReplaceReplacement>,
}

#[derive(Deserialize)]
pub struct ReplaceReplacement {
    pub original: String,
    pub display_value: String,
    pub entity_type: String,
    pub token_id: String,
    pub start: usize,
    pub end: usize,
}
```

The endpoint path is `/replace` (not `/v1/chat/completions`). This requires the SLM sidecar to expose this custom endpoint — see §9.

### 4.4 Token ID Reconciliation

The SLM returns a `token_id` in the response. The proxy validates that this matches the locally computed token ID for the entity index. If they match, use the SLM's value (canonical). If they diverge (e.g., the SLM uses a different scheme), log DEBUG and use the locally computed value. This keeps the proxy as the authority on token IDs.

**Alternative approach**: The proxy generates all token IDs locally and passes them to the SLM in the request:

```json
{
  "text": "...",
  "conversation_id": "...",
  "entity_start_index": 5
}
```

The SLM then assigns IDs starting from `entity_start_index`. This is simpler and keeps the SLM stateless on token ID generation. **This is the preferred approach** — eliminates reconciliation entirely.

---

## 5. Outbound Pipeline Redesign

### 5.1 Stage 0 — Entry Point

`process_request_body_async` in `pii/mod.rs` is the sole entry point. Its control flow becomes:

```
if tiers.slm:
    stage1_result = slm.replace(raw_text, conversation_id).await  // Stage 1
    t3_text = stage1_result.modified_text (or raw_text on failure)
    exclusion_zones = parse_pii_spans(t3_text)  // find <pii>...</pii> ranges
else:
    t3_text = raw_text
    exclusion_zones = []

if tiers.regex or tiers.ner:
    spans = detect_spans_with_exclusions(t3_text, exclusion_zones, locale).await  // Stage 2
    if tiers.slm and tiers.ner:
        low_conf_spans = spans.filter(|s| s.confidence < threshold)
        confirmed = slm.disambiguate(t3_text, low_conf_spans).await  // Stage 3
        spans = high_conf_spans + confirmed

apply_replacements(t3_text, spans, vault)  // atomic vault write + token format
inject_system_instruction(request_json)
```

### 5.2 Stage 1 — T3 First Pass

1. Call `slm.replace(text, conversation_id)` for each message text entry.
2. On success: the sidecar returns a `replacements` array with `original`, `display_value`, `entity_type`, `start`, `end`. The proxy **ignores** `modified_text` from the sidecar entirely.
3. The proxy reconstructs working text deterministically: sort replacements by `start` descending, apply substitutions right-to-left on the original text, inserting `<pii id="TOKEN_ID">display_value</pii>` where TOKEN_ID is computed locally from `(conversation_id, entity_index)`.
4. Vault-insert each replacement pair with the locally-computed token_id, tier=3, confidence=1.0.
5. Compute exclusion zones from the reconstructed text (positions of `<pii ...>...</pii>` tokens).
6. On sidecar failure: use raw text unchanged, no exclusion zones. T1/T2 run on the full original.

Right-to-left application preserves byte offsets for earlier spans when later spans are substituted first.

### 5.3 Stage 2 — T1 + T2 with Exclusion Zones

`detect_spans` gains a parameter: `exclusion_zones: &[(usize, usize)]`.

A span `[s, e]` is accepted iff for all exclusion zones `[s_i, e_i]`: `e <= s_i OR s >= e_i`.

T1 (regex) runs synchronously. T2 (GLiNER) runs concurrently via `tokio::spawn` as today. Both receive the same text and exclusion zones.

Merged spans proceed to vault write with token format: `<pii id="TOKEN_ID">DISPLAY_VALUE</pii>`.

### 5.4 Stage 3 — T3 Disambiguation

Unchanged role: low-confidence T1+T2 spans go to `/disambiguate`. Only triggered when `tiers.slm=true AND (tiers.regex OR tiers.ner)`. When T3 is enabled without T1/T2, disambiguation is skipped (no spans to disambiguate).

### 5.5 Vault Write — Unified Token Format

`SyntheticGenerator::get_or_create` returns a bare display-value `String` today. Under the new design, the return value must NOT change — callers in `main.rs` (`cmd_test_pii`) use the returned string directly for CLI table display, and that display must show the human-readable synthetic, not a raw XML token.

Instead, the XML wrapping is done at the call site in `replace_with_spans` (and the new Stage 1 vault-insert path).

**Token ID stability (contrarian challenge 6 resolved)**: Token IDs must be stable across runs for the same entity. Using `vault.mapping_count()` as entity_index introduces a race: T1 and T2 run concurrently and the insertion order determines which entity_index each entity receives. To eliminate this, entity_index is pre-assigned before any concurrency:

- All spans are collected (T1 synchronous, T2 concurrent) and merged into a final sorted list.
- The sorted list is traversed in `start`-offset order; each span receives `entity_index = current_vault_size + position_in_sorted_list`.
- This assignment happens inside `replace_with_spans` after the lock is taken, with the full sorted span list available. Order is deterministic: sorted by `span.start`.

New entries from Stage 1 (T3) are also assigned indices based on their `start` offset order from the sidecar response, computed before Stage 2 begins.

```rust
// In replace_with_spans (pii/mod.rs) — conceptual
let base_index = vault.mapping_count() as u64;
for (position, span) in sorted_spans.iter().enumerate() {
    let entity_index = base_index + position as u64;
    let display_value = SyntheticGenerator::get_or_create(vault, original, ...);
    let token_id = generate_token_id(conversation_id, entity_index);
    let xml_token = format!("<pii id=\"{token_id}\">{display_value}</pii>");
    vault.add_mapping_with_token_id(original, display_value, token_id, ...);
}
```

`SyntheticGenerator::get_or_create` signature is **unchanged**. A new `add_mapping_with_token_id` method is added to `PiiVault` alongside the existing `add_mapping`.

The `reverse_automaton` (Aho-Corasick) remains over **display values** for Level 5. Level 1 uses a separate `full_token_to_original: HashMap<String, String>` keyed on the exact emitted XML string. Levels 2 and 3 use `token_id_to_original` and `display_value_to_original` HashMaps.

---

## 6. Vault Structure Update

### 6.1 New VaultRecord Fields

```rust
pub struct VaultRecord {
    pub token_id: String,        // new: 8-char base62
    pub original: String,
    pub synthetic: String,       // the bare display_value (unchanged)
    pub display_value: String,   // same as synthetic; explicit field for clarity
    pub pii_type: PiiType,
    pub tier: u8,
    pub confidence: f32,
}
```

`display_value` is kept separate from the XML token string. The vault's reverse automaton key is the full XML token; the vault stores `original` (real PII) and `display_value` (synthetic) separately.

### 6.2 New Lookup Methods on PiiVault

```rust
pub fn get_by_token_id(&self, token_id: &str) -> Option<&str>  // → original
pub fn get_by_display_value(&self, display_value: &str) -> Option<&str>  // → original
```

These support cascade matching levels 2 and 3 (see §7).

### 6.3 Index Structures

Four new fields on `PiiVault`:

- `token_id_to_original: HashMap<String, String>` — keyed on TOKEN_ID (8 chars), value is `original`. Supports cascade Level 2.
- `display_value_to_original: HashMap<String, String>` — keyed on bare display value, value is `original`. Supports cascade Level 3.
- `full_token_to_original: HashMap<String, String>` — keyed on the exact emitted XML token string (`<pii id="TOKEN_ID">DISPLAY_VALUE</pii>`), value is `original`. Supports cascade Level 1 without Aho-Corasick overhead for the primary happy path.

All three are populated on every `add_mapping_with_token_id` call alongside the existing `synthetic_keys`/`original_values` vectors.

---

## 7. Inbound ReplacementBuffer — Cascade Matching

### 7.1 Architecture Change

The `ReplacementBuffer` gains a second operating mode running alongside the existing Aho-Corasick path:

- **XML-token path (Levels 1–4)**: scans for the literal 4-byte sequence `<pii`, accumulates until `</pii>`, then dispatches cascade matching.
- **Legacy Aho-Corasick path (Level 5)**: unchanged — 2-byte prefix trigger set built over display values (NOT over full XML token strings).

**Critical correction from contrarian**: The `trigger_prefixes` set must be built from the 2-byte prefixes of **display values only**, not of full XML token strings. Building it from XML tokens would put `['<', 'p']` in every vault, triggering holdback on virtually all prose. The vault's `max_synthetic_key_len` field continues to track display value length for the Level 5 window. The `reverse_automaton` (Aho-Corasick) for Level 5 is built over display values — not XML tokens.

### 7.2 Two Separate Automata

The vault maintains two distinct matching structures:

- `reverse_automaton`: Aho-Corasick over **display values** (bare synthetics). Used for Level 5. Built from `synthetic_keys` as today.
- Level 1–3 cascade: triggered by `<pii` literal scan, resolved via `token_id_to_original` and `display_value_to_original` HashMaps. No separate Aho-Corasick needed for Levels 1–3.

Level 1 (full XML token exact match) is handled by checking: does the accumulated token string exactly match what the proxy emitted? Since the proxy constructs tokens deterministically, a simple HashMap lookup keyed on the full token string suffices. An Aho-Corasick over XML tokens is not required.

### 7.3 Holdback Window

Two concurrent holdback windows:

1. **XML-token window**: active whenever `<pii` bytes appear in the buffer. Holdback = `<pii id="` (9) + 8 + `">` (2) + `max_display_value_len` + `</pii>` (6) bytes.
2. **Display-value window**: the existing `max_synthetic_key_len`-byte holdback, triggered by the 2-byte prefix set over display values. Unchanged behavior.

The buffer holds back `max(xml_window, display_value_window)` bytes when either trigger is active.

### 7.4 Cascade Matcher

When `</pii>` is found, the complete token is extracted and passed through:

**Level 1 — Full token exact match**: `full_token_to_original: HashMap<String, String>` keyed on the exact emitted XML token. O(1) lookup. Built on vault insert alongside `token_id_to_original`.

**Level 2 — ID-only match**: extract `id="TOKEN_ID"`, call `vault.get_by_token_id(token_id)`.

**Level 3 — Display value match**: extract inner text, call `vault.get_by_display_value(display_value)`.

**Level 4 — Hypothesis match (Part II stub)**: log WARN, pass token through unchanged.

**Level 5 — Bare display value scan**: the existing `replace_synthetics` Aho-Corasick fires on text that does not contain `<pii`. For text that slips through with the XML tags stripped, the buffer's display-value Aho-Corasick catches bare synthetics.

### 7.5 Streaming Correctness

The buffer must handle `<pii` split across SSE chunks. The XML-token holdback window ensures the buffer never flushes a partial `<pii` sequence. The display-value window handles split bare synthetics as before. Both windows operate simultaneously; the buffer flushes up to `text.len() - max(both_windows)` bytes on each delta.

---

## 8. System Instruction Injection

### 8.1 SYSTEM_REMINDER Replacement

The current `SYSTEM_REMINDER` constant (§-based instruction) is replaced with:

```rust
pub const SYSTEM_REMINDER: &str = "\
The user's message contains `<pii id=\"...\">...</pii>` elements. These are opaque \
placeholders for sensitive values. You must treat the content inside each `<pii>` element \
as a single atomic unit and reproduce it verbatim — including the XML tags and id attribute \
— whenever you reference or repeat that value. Never rephrase, abbreviate, or split a \
`<pii>` element across multiple tokens.";
```

### 8.2 Injection Point

**Codebase finding**: `inject_system_instruction` already exists in `pii/mod.rs` and is already provider-aware — it handles Anthropic (top-level `system` field), OpenAI (`messages[0]` with `role=system`), and Google (currently no-op). A wrapper `inject_system_instruction_into_body` exists in `c2u.rs`.

The injection is currently gated at `c2u.rs` line 324 on `p.pipeline.slm_standalone && p.mode == PiiMode::Replace`. This gate must change to simply `p.mode == PiiMode::Replace` — system instruction injection applies whenever PII replacement is active, regardless of which tiers are enabled. The `SYSTEM_REMINDER` text is updated as described in §8.1.

No structural change to the injection mechanism is needed. Only the gate condition and the reminder text change.

Google support remains a no-op in Part I. Google's `systemInstruction` field has a different schema (`{parts: [{text: "..."}]}`); adding support is a minor follow-up not scoped to this feature.

---

## 9. Packaging Changes

### 9.1 Current State

`postinstall` script checks for `$SHARE_DIR/llama-server` and copies it to `~/Library/Application Support/privacyclaw/bin/llama-server`. Homebrew formula has `depends_on "llama.cpp"`.

The T3 sidecar today is a generic `llama-server` binary serving the standard OpenAI-compatible `/v1/chat/completions` endpoint. The new `/replace` endpoint does **not** exist in `llama-server`.

### 9.2 Protocol Decision for /replace

The contrarian challenge correctly identifies that Option C (prompted LLM producing structured JSON with byte offsets) is not reliable. Small LLMs cannot count bytes precisely. The spec's "CRITICAL" language on explicit span reporting implies a deterministic service.

**Decision: Option A — Wrapper sidecar**, but with a simplified interface that avoids requiring byte offsets from the LLM.

The `/replace` endpoint is implemented in a thin sidecar (Python recommended for the SLM binding layer) that:

1. Accepts `{"text": "...", "conversation_id": "..."}`.
2. Runs the LLM with a prompt requesting **only the list of detected PII strings** (not byte offsets, not modified_text): `["Anne Nicole", "bob@corp.com"]`.
3. For each detected PII string, finds **all** occurrences in the original `text` using `str.find()` iterated from the last-found position — not just the first. Each occurrence produces a separate replacement entry with its own `start`/`end`.
4. Generates a placeholder `display_value` (the proxy overrides this with `SyntheticGenerator` output in all cases — the sidecar's `display_value` is ignored).
5. Returns the full `ReplaceResponse` structure with one entry per occurrence.

**Duplicate entity handling**: If "Anne Nicole" appears 3 times in the text, the sidecar returns 3 replacement entries with the same `original` but different `start`/`end`. The proxy maps all 3 to the same vault entry (idempotent `SyntheticGenerator::get_or_create`) and therefore the same `token_id` and `display_value`. All 3 occurrences are replaced with the same XML token — which is correct: the same entity maps to the same synthetic throughout the conversation.

This keeps LLM output responsibility minimal (a flat list of strings, not JSON with offsets), while the sidecar handles all deterministic parts. LLM string extraction is far more reliable than LLM byte-offset computation.

**Packaging impact**: The sidecar is a Python script (`privacyclaw-slm-sidecar`) bundled alongside `llama-server`. The `postinstall` script is extended to install it. Homebrew formula gains a Python script installation step. This is a concrete packaging change.

**Scope note**: The sidecar implementation itself (the Python script and its prompt) is out of scope for the Rust proxy changes in Part I. The proxy defines the HTTP API contract (`ReplaceResponse`); the sidecar fulfills it. Part I delivers the proxy side; sidecar implementation is a parallel work item.

### 9.3 Modified Text Reconstruction

**Critical correction from contrarian**: The proxy must NOT use the LLM's `modified_text` directly as the T1/T2 input. The LLM may rewrite surrounding prose, add punctuation, or produce non-well-formed XML. Instead:

The proxy reconstructs `modified_text` deterministically from the `replacements` array:
1. Sort replacements by `start` ascending.
2. Walk the original `text`, replacing spans `[start, end)` with `<pii id="TOKEN_ID">display_value</pii>`.
3. Token IDs are computed locally by the proxy from `(conversation_id, entity_index)`.
4. The LLM's `modified_text` field in the response is **ignored entirely**.

This makes Stage 1 output fully deterministic and independent of LLM text generation quality. The sidecar's `modified_text` field is vestigial in this design — the response struct retains it for forward compatibility but the proxy discards it.

### 9.4 Model Weights

The sidecar reuses the same GGUF model used for `/disambiguate`. No new weights are required.

### 9.5 Dev Packaging

`cargo build` continues to work without the sidecar. The sidecar is optional for T3 functionality — when unavailable, Stage 1 is skipped (same fail-open behavior as today when the SLM endpoint is unreachable).

---

## 10. Data Flow — End to End

```
User message (raw text)
        │
        ▼
[T3 enabled?] ─── yes ──→ SlmSidecar::replace(text, conv_id)
        │                          │
        │                          ▼
        │                  ReplaceResponse {
        │                    modified_text,
        │                    replacements[]  ← vault-insert (tier=3)
        │                  }
        │                          │
        │              parse exclusion zones from modified_text
        │                          │
        ▼                          ▼
[T1/T2 enabled?] ──────────→ detect_spans_with_exclusions(modified_text, zones)
        │                          │
        │                          ▼
        │                  merge(t1_spans, t2_spans)
        │                          │
        │              [T3 enabled?] → /disambiguate(low_conf_spans)
        │                          │
        │                          ▼
        │                  vault-insert T1/T2 spans (tier=1/2)
        │
        ▼
apply token format: <pii id="TOKEN_ID">DISPLAY_VALUE</pii>
inject SYSTEM_REMINDER into request
        │
        ▼
    Upstream LLM
        │
        ▼
    SSE stream (inbound)
        │
        ▼
ReplacementBuffer::process_delta()
  trigger: "<pii"
  hold until "</pii>"
  cascade L1→L2→L3→L4(stub)→L5
        │
        ▼
    Client (original values restored)
```

---

## 11. Integration Points with Existing Code

| Component | Change Required |
|---|---|
| `pii/vault.rs` | Add `token_id`, `display_value` to `VaultRecord` with `#[serde(default)]`; add `token_id_to_original` and `display_value_to_original` HashMaps; add `add_mapping_with_token_id`; add `get_by_token_id`, `get_by_display_value`; add `generate_token_id` helper |
| `pii/mod.rs` | Restructure `process_request_body_async` for T3-first flow; remove `slm_standalone` field from `PiiPipeline`; rewrite `validate_pii_tiers` to allow T3+T1; update `SYSTEM_REMINDER` text; thread exclusion zones into `detect_spans`; wrap XML token at call sites in `replace_with_spans` |
| `pii/tier3.rs` | Add `SlmSidecar::replace()` using `/v1/chat/completions` with structured prompt; add `ReplaceResponse`/`ReplaceReplacement` structs; keep `detect_and_rewrite` and `extract_token_pairs` intact (still used by existing tests) |
| `pii/buffer.rs` | Add `<pii` literal trigger alongside existing 2-byte prefix trigger; add cascade matcher (L1 via existing Aho-Corasick, L2/L3 via new vault methods); holdback window updated to cover max XML token length |
| `pii/synth.rs` | **No signature change** to `get_or_create`; XML wrapping moved to call sites |
| `config.rs` | Rewrite `validate_pii_tiers` to relax T3+T1 restriction; `is_t3_standalone` unchanged |
| `proxy/intercept/c2u.rs` | Change system-instruction gate from `slm_standalone && Replace` to just `Replace`; delete `slm_standalone` reference |
| `proxy/intercept/pii_sse.rs` | No change — operates on extracted text deltas |
| `Cargo.toml` | No new crates — `sha2` already present; `hmac` not needed |
| `packaging/postinstall` | Extend to install `privacyclaw-slm-sidecar` Python script alongside `llama-server` |
| `docs/` | Move `.claude/workflow/pii-pipeline-v2/design.md` → `docs/pii-pipeline-v2.md` |

---

## 12. Open Questions

1. **SLM sidecar protocol for /replace**: Option C (prompt adapter via `/v1/chat/completions`) is the design choice for Part I — no new sidecar binary. This requires the SLM to reliably output JSON matching `ReplaceResponse`. Is this acceptable, or does the spec mandate a native `/replace` REST endpoint (requiring a wrapper sidecar)?

2. **WS PiiDetected event format**: The `PiiDetection.synthetic` field is sent to the dashboard in `WsEvent::PiiDetected`. Under the new design, the "synthetic" value for T1/T2 detections is the full XML token `<pii id="...">display</pii>`. The dashboard will show this in the PII replacement UI. Is that acceptable, or should the dashboard receive the bare `display_value` separately?

3. **Vault backward compat on upgrade**: Existing vault records (within their 24h TTL) lack `token_id`. Cascade Level 2 will fail for those records (empty token_id won't match anything). The 24h TTL means this self-heals within one day. Is a migration step needed or is natural TTL rotation acceptable?

4. **Level 5 dual-trigger during rollout**: Post-deployment, old conversations (pre-upgrade) may have bare synthetics in the vault. The new buffer must fire the old prefix-based Aho-Corasick for Level 5. This dual-mode is already planned (§7.4). Confirm this is acceptable rather than a schema-version flag approach.

---

## 13. Deprecation of detect_and_rewrite

**Contrarian challenge 5 resolved**: `detect_and_rewrite`, `extract_token_pairs`, and `SYSTEM_PROMPT_STANDALONE` in `tier3.rs` must be explicitly retired in this feature, not kept as dead code. The `cargo clippy -- -D warnings` policy prohibits dead code.

Retirement plan:
- `process_body_t3_standalone` in `pii/mod.rs` is deleted (the new T3-only path in `process_request_body_async` replaces it entirely).
- `detect_and_rewrite` and `extract_token_pairs` in `tier3.rs` are deleted.
- `SYSTEM_PROMPT_STANDALONE` in `tier3.rs` is deleted.
- The 5 unit tests for `detect_and_rewrite` (`detect_and_rewrite_sends_correct_max_tokens_and_system_prompt`, `detect_and_rewrite_parses_well_formed_response`, `detect_and_rewrite_timeout_returns_none`, `detect_and_rewrite_http_500_returns_none`, `detect_and_rewrite_no_section_sign_returns_none`) are deleted and replaced by new tests for `SlmSidecar::replace()`.
- The 2 tests for `slm_standalone` flag (`pipeline_slm_standalone_flag_true_and_tier2_none`, `pipeline_full_stack_slm_standalone_false`) are replaced by tests for the new tier-matrix routing logic.

This is a planned breaking change within the PII subsystem. The existing `§value§` format is fully retired. Vault entries from prior sessions using the `§` format will have their `token_id` field empty (defaulted) and will only match via Level 5 (display-value Aho-Corasick on bare synthetics), which is correct fallback behavior during the TTL window.

## 14. Dashboard Display of PiiDetected

**Contrarian challenge 7 resolved**: The `WsEvent::PiiDetected` event carries a `synthetic` field. Under the new design, `PiiDetection.synthetic` is populated with the `display_value` (bare synthetic), not the full XML token. The XML token is only written into the request body text; it is not stored in `PiiDetection.synthetic`.

Specifically, `replace_with_spans` populates `PiiDetection` with:
- `original`: the real PII substring
- `synthetic`: the `display_value` (e.g., `Maria Blinke`) — what the dashboard shows
- `tier`, `confidence`, `entity_type` as before

The XML token is assembled separately and inserted into the request body. This keeps the dashboard human-readable without exposing the XML wrapper to the UI.

## 15. Multi-Turn Token ID Stability

**Contrarian challenge 4 documented**: In multi-turn conversations, token IDs assigned in Turn 1 persist in the vault for 24 hours. When the upstream LLM echoes a `<pii>` token from Turn 1 in its Turn 3 response, the cascade correctly resolves it via the vault's `token_id_to_original` map.

The system instruction is injected per-request. The LLM's context window contains the prior turns (including the XML tokens), so it sees the tokens from Turn 1 as part of the conversation history. No additional mechanism is needed — the LLM is instructed to reproduce tokens verbatim and has them in context.

This behavior relies on the client sending full conversation history in each request (standard for chat APIs). Stateless single-turn requests have no cross-turn token reference risk by definition.

## 16. Non-Goals (Part I)

- Surface Form Oracle (hypothesis generation, scoring, MVM training)
- Level 4 cascade matching implementation
- `/hypothesize` SLM endpoint
- Streaming SSE replacement during partial `<pii>` token accumulation (full token must arrive before reversal)
- Multi-language support changes (existing `Locale` system unchanged)
- Google `systemInstruction` field injection (existing no-op behavior retained)
