# Change: Adaptive PII Protection — T3-First Pipeline (Part I)

## Why

The current PII pipeline runs T3 (SLM) as a disambiguator on text already
processed by T1/T2, which means the SLM sees synthetic-contaminated text and
cannot reliably detect contextual PII. Synthetic values are indistinguishable
from real values, causing double-replacement and vault corruption. Moving T3
to a first-pass position on raw text resolves this and enables pattern-free
PII detection (safe codes, partial card fragments).

## What Changes

- **Token format**: All synthetic replacements now use `<pii id="TOKEN_ID">DISPLAY_VALUE</pii>` (XML tags) instead of bare synthetics. TOKEN_ID is an 8-char base62 string derived from SHA-256(conversation_id + ":" + entity_index).
- **Pipeline order**: T3 runs first on raw text; T1/T2 run on T3-modified text with exclusion zones covering already-replaced spans. **BREAKING** (pipeline execution order and token format).
- **Config validation**: `validate_pii_tiers` rewritten — T3+T1 (no T2) is now valid. The guard "T3 requires T1+T2" is removed. T2-without-T1 remains invalid.
- **`slm_standalone` field removed** from `PiiPipeline`; behaviour is fully expressed by the tier matrix.
- **New SLM endpoint**: `SlmSidecar::replace()` calls `/replace` (not `/v1/chat/completions`). Returns structured `replacements[]` array; proxy reconstructs modified text deterministically (LLM's `modified_text` ignored).
- **Dead code retired**: `detect_and_rewrite`, `extract_token_pairs`, `SYSTEM_PROMPT_STANDALONE` deleted. All associated tests replaced.
- **ReplacementBuffer**: dual-trigger holdback — XML-token path (`<pii` literal scan) + existing Aho-Corasick over display values (Level 5).
- **Vault**: gains `token_id`, `display_value` fields on `VaultRecord`; three new lookup HashMaps; `add_mapping_with_token_id` method; `generate_token_id` helper.
- **System instruction gate**: broadened from `slm_standalone && Replace` to just `Replace`; `SYSTEM_REMINDER` text updated to describe XML token format.
- **Dashboard**: `PiiDetection.synthetic` carries bare `display_value` (not XML token) — human-readable.
- **Packaging**: `postinstall` extended to install `privacyclaw-slm-sidecar` Python script alongside `llama-server`. (Sidecar implementation is parallel work, not in Rust scope.)

## Impact

- Affected specs: `pii-pipeline`, `pii-vault`
- Affected code: `src/pii/vault.rs`, `src/pii/mod.rs`, `src/pii/tier3.rs`, `src/pii/buffer.rs`, `src/config.rs`, `src/proxy/intercept/c2u.rs`, `packaging/postinstall`
- Breaking: token format change means any vault entries from prior sessions (with bare synthetics) will fall through to Level 5 (Aho-Corasick on display values) — correct fallback during 24h TTL window
- No new Cargo dependencies — `sha2` already present
