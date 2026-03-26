## ADDED Requirements

### Requirement: Tier Activation Matrix Enforcement

`ConfigManager::patch` SHALL reject tier configurations that violate the activation matrix.
The only invalid combination is Tier 2 (NER) enabled without Tier 1 (regex). All other
combinations (including T3 without T2, T3+T1 without T2) are valid.

An error SHALL be returned with a message that mentions the violated dependency (e.g.,
references "tier 1", "tier1", "regex", or "requires").

#### Scenario: Tier 2 without Tier 1 rejected

- **WHEN** `ConfigManager::patch` is called with `{ pii: { tiers: { ner: true } } }`
- **AND** the current config has `regex: false`
- **THEN** the patch returns an `Err`
- **AND** the error message mentions the Tier 1 / regex dependency

#### Scenario: Tier 3 without Tier 2 is valid

- **WHEN** `ConfigManager::patch` is called with `{ pii: { tiers: { slm: true } } }`
- **AND** the current config has `regex: true, ner: false`
- **THEN** the patch returns `Ok`
- **AND** the configuration is updated successfully
