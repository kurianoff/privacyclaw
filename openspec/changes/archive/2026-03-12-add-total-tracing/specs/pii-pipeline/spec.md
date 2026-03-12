## ADDED Requirements

### Requirement: Tier 1 Detector Tracing

The `Tier1Detector` SHALL emit `tracing` calls at DEBUG and INFO level for detection results and at TRACE level for per-pattern branch decisions.

#### Scenario: DEBUG record per detected span

- **WHEN** `Tier1Detector::detect` (or `find_all`) matches any PII span in the input text
- **THEN** a `tracing::debug!` call is emitted for each span containing fields: `entity_type`, `span_start`, `span_end`, `confidence`, `text_len`
- **AND** the call is made after all patterns for that entity type have run (not once per regex attempt)

#### Scenario: INFO summary after full detection run

- **WHEN** `Tier1Detector::detect` completes processing all 14+ entity patterns against the input text
- **THEN** a `tracing::info!` call is emitted with fields: `entity_type = "tier1"`, `count` (total spans found), `text_len`

#### Scenario: TRACE for each pattern attempt

- **WHEN** `RUST_LOG=trace` is set
- **THEN** for each of the 14+ entity patterns, a `tracing::trace!` call is emitted with the pattern name and whether it matched (boolean field `matched`)
- **AND** for `CreditCard` patterns, an additional `tracing::trace!` call records `entity_type = "CreditCard"`, `span_start`, `span_end`, and `verdict` (`"pass"` or `"fail_luhn"`)

---

### Requirement: Synthetic Generator Tracing

The `SyntheticGenerator` SHALL emit `tracing` calls at DEBUG level for cache hits and at INFO level for first-time synthetic value generation.

#### Scenario: INFO on first synthetic generation

- **WHEN** `SyntheticGenerator::get_or_create` is called for an `(entity_type, original)` pair not previously seen in the current conversation
- **THEN** a `tracing::info!` call is emitted with fields: `conv_id`, `entity_type`, `original`, `synthetic`

#### Scenario: DEBUG on cache hit

- **WHEN** `SyntheticGenerator::get_or_create` is called for an `(entity_type, original)` pair already in the cache
- **THEN** a `tracing::debug!` call is emitted with fields: `conv_id`, `entity_type`, `original_len`
- **AND** `original` and `synthetic` values are NOT included in the DEBUG record (to limit payload exposure at non-INFO levels)

#### Scenario: TRACE on generate internals

- **WHEN** `RUST_LOG=trace` is set and `SyntheticGenerator::generate` is called
- **THEN** a `tracing::trace!` call is emitted on entry with `entity_type` and `input_len`
- **AND** a `tracing::trace!` call is emitted on return with `entity_type` and `synthetic`

---

### Requirement: Replacement Buffer Tracing

The `ReplacementBuffer` SHALL emit `tracing` calls at DEBUG level for buffer state transitions and at TRACE level for per-character match decisions.

#### Scenario: DEBUG record on every process_delta call

- **WHEN** `ReplacementBuffer::process_delta` is called with any input
- **THEN** a `tracing::debug!` call is emitted with fields: `text_len`, `holdback_len` (before processing), `flushed_len` (bytes written to output this call)

#### Scenario: TRACE on synthetic-token boundary match

- **WHEN** `RUST_LOG=trace` is set
- **THEN** for each synthetic token prefix match or abandonment decision in `process_delta`, a `tracing::trace!` call is emitted with fields: `matched_key` (the synthetic token string or prefix), `span_start`, `span_end` (positions within the current delta)

#### Scenario: INFO when flush_remaining produces output

- **WHEN** `ReplacementBuffer::flush_remaining` is called and the holdback buffer is non-empty
- **THEN** a `tracing::info!` call is emitted with field: `flushed_len` (number of bytes flushed)

#### Scenario: No log call when buffer and flush are both empty

- **WHEN** `process_delta` is called with empty input and `flush_remaining` is called with an empty holdback
- **THEN** no tracing macro is invoked (avoiding empty-record noise)

---

### Requirement: PII Vault Tracing

The `PiiVault` SHALL emit `tracing` calls at DEBUG level for lookup results and at INFO/WARN level for structural events (insert, reload, error).

#### Scenario: DEBUG on vault lookup

- **WHEN** `PiiVault::get_synthetic` or `PiiVault::get_original` is called
- **THEN** a `tracing::debug!` call is emitted with fields: `conv_id`, `entity_type`, `hit` (bool)
- **AND** the actual key/value strings are NOT included in the DEBUG record

#### Scenario: DEBUG on vault insert

- **WHEN** `PiiVault::insert` adds a new mapping
- **THEN** a `tracing::debug!` call is emitted with fields: `conv_id`, `entity_type`, `original`, `synthetic`, `mapping_count` (total entries after insert)

#### Scenario: WARN on vault reload

- **WHEN** `PiiVault::reload` loads mappings from disk
- **THEN** a `tracing::warn!` call is emitted on success with fields: `vault_path`, `mapping_count`
- **AND** a `tracing::warn!` call is emitted on file error with fields: `vault_path`, `err = %e`
- **AND** no format-string interpolation is used in any of these calls

#### Scenario: TRACE on vault reload per-entry

- **WHEN** `RUST_LOG=trace` is set and `PiiVault::reload` restores entries from disk
- **THEN** a `tracing::trace!` call is emitted for each key-value pair restored, with fields: `entity_type`, `original_len`, `synthetic_len`

---

### Requirement: detect_spans Per-Tier Summary

The `detect_spans` orchestrator in `src/pii/mod.rs` SHALL emit a DEBUG summary for each tier that runs.

#### Scenario: DEBUG count per active tier

- **WHEN** `detect_spans` invokes any of Tier 1, Tier 2, or Tier 3
- **THEN** after each tier completes, a `tracing::debug!` call is emitted with fields: `tier` (1, 2, or 3), `count` (spans returned by that tier)
- **AND** a final `tracing::debug!` call is emitted after deduplication with `count` = total unique spans

#### Scenario: No log call when tier is disabled

- **WHEN** a tier is disabled in config (`pii.tiers.ner = false`)
- **THEN** no `tracing` call is emitted for that tier's result
