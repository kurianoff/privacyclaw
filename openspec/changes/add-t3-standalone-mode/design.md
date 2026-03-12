# Design: Tier 3 Standalone Mode

## Overview

This document covers the technical design for `add-t3-standalone-mode`. Four areas require detailed treatment: the standalone data flow, the `extract_token_pairs` algorithm, the system instruction strategy per provider, vault population in standalone mode, and the Google provider limitation.

---

## (a) T3 Standalone Data Flow

```
Client request body (JSON)
        │
        ▼
┌───────────────────────────┐
│  process_body_t3_standalone│  (new, in pii/mod.rs)
└────────────┬──────────────┘
             │  extract all message content texts
             ▼
    for each text:
┌───────────────────────────────┐
│  SlmSidecar::detect_and_rewrite│  (new, in pii/tier3.rs)
│  ─ SYSTEM_PROMPT_STANDALONE   │
│  ─ dynamic max_tokens          │
│  → (rewritten_text, pairs)     │
└─────────────┬─────────────────┘
              │  pairs = Vec<(original_span, synthetic_span)>
              ▼
┌──────────────────────┐
│  populate vault       │  vault.get_or_create per pair
│  replace in text      │  simple string replace of original→synthetic
└──────────┬───────────┘
           │  modified JSON value
           ▼
┌─────────────────────────────────┐
│  inject_system_instruction       │  (new, in pii/mod.rs)
│  adds §-reminder to system field │
└──────────┬──────────────────────┘
           │  final body bytes
           ▼
   forwarded to upstream LLM API

Upstream LLM API response (SSE)
        │
        ▼
┌───────────────────────────┐
│  ReplacementBuffer (existing)│  replaces synthetic→original
└──────────────────────────┘
        │
        ▼
  Client receives original PII restored
```

**Key invariants:**
- Tier 1 and Tier 2 detection are skipped entirely; only `detect_and_rewrite` runs.
- The vault is populated from `extract_token_pairs` output, exactly as it is in the Tier 1+2 path.
- The `ReplacementBuffer` operates identically in standalone mode — no changes needed.
- If `detect_and_rewrite` returns `None` (SLM timeout, HTTP error, or no `§` markers in output), the original body is forwarded unmodified and the vault remains empty for this request.

---

## (b) `extract_token_pairs` Algorithm and Abort Conditions

### Input
- `original: &str` — the original message text sent to the SLM.
- `rewritten: &str` — the SLM output text with `§value§` markers around detected PII values.

### Algorithm

```
pairs = []
i_orig = 0
i_rewr = 0

while i_rewr < rewritten.len():
    if rewritten[i_rewr] == '§':
        // find closing §
        end = rewritten.find('§', i_rewr + 1)?  // abort if no closing §
        inner = rewritten[i_rewr+1 .. end]       // the SLM's wrapper content
        // locate inner in original starting from i_orig
        pos = original[i_orig..].find(inner)?    // abort token if not found
        orig_start = i_orig + pos
        orig_end   = orig_start + inner.len()
        pairs.push((original[orig_start..orig_end], synthetic_for(inner)))
        i_orig = orig_end
        i_rewr = end + 1  // skip closing §
    else:
        // advance both pointers in sync over non-PII text
        advance_until_next_marker(i_orig, i_rewr)
```

### Synthetic value (vault storage schema)

The vault stores: `original → §original§` directly. That is, `vault.add_mapping(original_span, "§{original_span}§", PiiType::Unknown, 3)`.

- `original_to_synthetic["Peter"] = "§Peter§"` — used by outbound path to replace `Peter` with `§Peter§`
- `synthetic_keys` contains `"§Peter§"` — used by inbound `ReplacementBuffer` AhoCorasick to replace `§Peter§` back with `Peter`

Do NOT call `SyntheticGenerator` or `vault.get_or_create` for T3 standalone pairs — those generate random synthetic names. In T3 standalone mode the SLM chooses the token form; the proxy must store exactly what the SLM will echo back (`§Peter§`), not an unrelated generated name.

### Abort conditions
1. **No `§` in rewritten output**: return empty `Vec` (SLM produced no markers — treat as no detection found).
2. **Unclosed `§` marker**: skip the malformed token, continue.
3. **Inner text not found in original**: skip that token (alignment failure), increment failure count.
4. **>50% of identified tokens fail alignment**: log `WARN`, return partial results collected so far.
5. **SLM timeout or HTTP error**: `detect_and_rewrite` returns `None`; caller forwards original body unchanged.

Condition 4 uses: `failures / total_markers_found > 0.5` where `total_markers_found` is the count of `§` opening characters in the rewritten text.

---

## (c) System Instruction Content (per Provider)

### Purpose

When T3 standalone replaces PII with synthetic tokens, the upstream LLM sees synthetic names like `"xk_alice_7f2a"`. Without guidance, the LLM may paraphrase, correct, or hallucinate these tokens. The system instruction tells the model to treat `§`-wrapped tokens as opaque identifiers.

### Content (`SYSTEM_REMINDER` constant)

```
Some values in this conversation have been replaced with privacy tokens of the form §token§.
Treat each §token§ as an opaque identifier: reproduce it exactly as written,
do not paraphrase, expand, or modify it. When referring to a previously introduced token,
use the exact same §token§ string.
```

Note: by the time the system instruction is injected, the body has already been through `process_body_t3_standalone` which replaced originals with vault synthetics. The `§token§` form described in the reminder matches the synthetic tokens stored in the vault (synthetics generated from the `§`-wrapped SLM output contain the marker character as prefix/suffix to make them recognisable to `ReplacementBuffer`).

### Anthropic

The top-level `system` field in the Anthropic Messages API is a plain string:

```json
{ "system": "You are a helpful assistant.", "messages": [...] }
```

`inject_system_instruction` appends to this string:

```
\n\n<system-reminder>\n{SYSTEM_REMINDER}\n</system-reminder>
```

If the `system` field is absent, it is created as a string containing only the reminder block. If `system` is not a string (e.g., it is an array of content blocks — a valid Anthropic extension), the function returns `false` and logs a `WARN`.

### OpenAI

The OpenAI Chat Completions API uses a `messages` array. The system instruction is the content of the first `{"role":"system"}` message.

`inject_system_instruction`:
1. Iterates `messages` array for the first entry with `"role": "system"`.
2. Appends `\n\n{SYSTEM_REMINDER}` to the `"content"` string of that message.
3. If no system message exists, inserts `{"role": "system", "content": SYSTEM_REMINDER}` at index 0.
4. Returns `true`.

If `messages` is absent or not an array, returns `false` and logs `WARN`.

### Google (Gemini)

Google Gemini uses a separate `systemInstruction` field with a `parts` array schema, incompatible with the string-append approach:

```json
{ "systemInstruction": { "parts": [{ "text": "..." }] }, "contents": [...] }
```

`inject_system_instruction` returns `false` for `Provider::Google` without modifying the body. A `DEBUG` log entry notes the skip. Support for Google can be added in a follow-on change by constructing the appropriate `parts` entry.

---

## (d) Vault Population Strategy in Standalone Mode

In standalone mode, the vault is populated from the `(original_span, synthetic)` pairs produced by `extract_token_pairs` + `vault.get_or_create`:

```rust
for (original_span, _sidecar_suggested) in pairs {
    let synthetic = vault.get_or_create(&original_span, PiiType::Unknown);
    // apply replacement: body_text.replace(&original_span, &synthetic)
}
```

`PiiType::Unknown` is used because the SLM does not return a structured entity type — it only wraps spans. This causes the `SyntheticGenerator` to fall back to the generic format-preserving generator (random alphanumeric of the same length). A future enhancement could parse the SLM's context to infer the entity type.

The vault's Aho-Corasick automaton is rebuilt after all pairs are inserted (single `rebuild()` call), matching the existing Tier 1+2 path. The `ReplacementBuffer` then consumes the vault identically regardless of which tier populated it.

### Idempotency

`vault.get_or_create` is idempotent: if the SLM detects the same span in a subsequent turn of a multi-turn conversation, the same vault synthetic is returned. This ensures the `ReplacementBuffer` correctly reverses all occurrences.

---

## (e) Google Provider Limitation

The Gemini API `systemInstruction` field uses a structured multi-part schema. Appending a plain string to it would require either:
1. Modifying the last `text` part in `systemInstruction.parts`, or
2. Appending a new part object.

Neither approach is safe without understanding whether the caller has already structured the `systemInstruction` deliberately. The conservative decision is to skip the injection entirely for `Provider::Google` and return `false`.

Consequence: when T3 standalone is used with a Google Gemini endpoint, the `§`-wrapped synthetic tokens are forwarded without a reminder to the model. The `ReplacementBuffer` still reverses them on the response path. The risk is that Gemini may paraphrase or ignore the synthetic tokens, leading to incomplete reversal. This is documented as a known limitation.

If Google support is required, a follow-on change should add:
```rust
if let Some(parts) = value["systemInstruction"]["parts"].as_array_mut() {
    parts.push(serde_json::json!({"text": SYSTEM_REMINDER}));
    return true;
}
// else create the field
value["systemInstruction"] = serde_json::json!({"parts": [{"text": SYSTEM_REMINDER}]});
true
```

This is out of scope for this change.
