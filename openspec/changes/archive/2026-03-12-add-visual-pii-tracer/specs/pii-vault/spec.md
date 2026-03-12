## ADDED Requirements

### Requirement: Vault Confidence Storage

`PiiVault` SHALL store a `confidence: f32` value alongside each mapping. The value SHALL be sourced from the `PiiDetection.confidence` produced by the detection pipeline and persisted in `StoredVaultRecord.confidence`.

`PiiVault::add_mapping` and `insert_mapping_raw` SHALL accept a `confidence: f32` parameter. The vault SHALL maintain a `confidences: Vec<f32>` parallel vec alongside the existing `tiers`, `pii_types`, `original_values`, and `synthetic_keys` vecs.

#### Scenario: Confidence stored with mapping

- **WHEN** `add_mapping(original, synthetic, pii_type, tier, confidence)` is called
- **THEN** `confidences[i]` equals the passed confidence value for the new mapping at index `i`
- **AND** all parallel vecs remain the same length

#### Scenario: Confidence available in quints iterator

- **WHEN** the vault's `quints()` iterator is called
- **THEN** each element is `(&original, &synthetic, &pii_type_label, tier, confidence)`
- **AND** the confidence value matches what was passed to `add_mapping`

#### Scenario: Legacy record loaded without confidence

- **WHEN** a `StoredVaultRecord` with `confidence: None` is loaded via `from_records`
- **THEN** `confidences[i]` is set to `0.0` (sentinel for "confidence unknown")
- **AND** the mapping is otherwise functional

---

### Requirement: Vault Persistence Includes Confidence

`Store::save_vault` SHALL persist confidence values in each `StoredVaultRecord`. On reload, confidence SHALL be restored from storage.

#### Scenario: Round-trip confidence preservation

- **WHEN** a vault with confidence values is saved and then reloaded
- **THEN** each mapping's confidence value equals the original value
- **AND** the `quints()` iterator reflects the reloaded values
