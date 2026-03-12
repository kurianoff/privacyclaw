## ADDED Requirements

### Requirement: Synthetic Key Prefix Iterator

`PiiVault` SHALL expose a `synthetic_key_prefixes()` method that returns an iterator over the first 2 bytes of each synthetic key. Keys shorter than 2 bytes SHALL be skipped. This method is used by `ReplacementBuffer` to build a 2-byte prefix trigger set, replacing the previous single-character trigger set.

#### Scenario: Prefix iterator returns correct 2-byte slices

- **GIVEN** a vault containing synthetic keys `"fd1a2b:3c4d::1"`, `"alice.smith@example.com"`, `"10.23.45.67"`
- **WHEN** `synthetic_key_prefixes()` is called
- **THEN** the iterator yields `[b'f', b'd']`, `[b'a', b'l']`, `[b'1', b'0']` (order unspecified)

#### Scenario: Keys shorter than 2 bytes are skipped

- **GIVEN** a vault that somehow contains a 1-byte synthetic key `"x"`
- **WHEN** `synthetic_key_prefixes()` is called
- **THEN** that entry is excluded from the iterator result

### Requirement: IPv6 Synthetic Key Length

The synthetic IPv6 generator (`gen_ipv6()`) SHALL produce values using exactly 2 random 16-bit hex groups in the format `fd{g1}:{g2}::1`, resulting in a key length of at most 16 characters. This replaces the previous 7-group format which produced ~39-character keys that inflated `max_synthetic_key_len` and caused `ReplacementBuffer` to hold back streaming response text unnecessarily.

#### Scenario: IPv6 synthetic is short enough not to stall the buffer

- **GIVEN** a vault with only IPv6 mappings
- **WHEN** `max_synthetic_key_len` is queried
- **THEN** the value is ≤ 16

#### Scenario: IPv6 synthetic is still unique-local format

- **GIVEN** `gen_ipv6()` is called
- **THEN** the result starts with `fd` (RFC 4193 unique local prefix)
- **AND** the result ends with `::1`
