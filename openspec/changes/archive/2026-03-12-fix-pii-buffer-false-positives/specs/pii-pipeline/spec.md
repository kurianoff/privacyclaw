## ADDED Requirements

### Requirement: Tier 1 IPv6 Detection Precision

The Tier 1 IPv6 detector SHALL reject false positives caused by Rust module path separators and other `::` occurrences in source code. A detected IPv6 candidate SHALL only be accepted if it passes the `ipv6_valid` post-match validator.

**Validator rules (all must hold):**
- The match contains at least 2 colon (`:`) characters.
- No colon-separated segment in the match is longer than 4 characters. (Rust identifiers like `vault`, `buffer`, `pii` exceed 4 chars and are therefore rejected.)

The IPv6 regex SHALL use a negative lookbehind `(?<![:\w])` to prevent matching inside larger identifiers.

#### Scenario: Rust path separator not detected as IPv6

- **GIVEN** the proxy is in PII-replace mode
- **WHEN** the user sends a message containing `use crate::pii::vault::PiiVault;`
- **THEN** `Tier1Detector::detect()` returns zero spans with `entity_type == IpV6`
- **AND** the text is forwarded to the LLM unmodified for that span

#### Scenario: Double-colon identifier not detected as IPv6

- **GIVEN** the proxy is in PII-replace mode
- **WHEN** the user sends a message containing `foo::bar` or `MyModule::method`
- **THEN** no IpV6 span is detected

#### Scenario: Abbreviated IPv6 still detected

- **WHEN** the user sends a message containing `fe80::1` or `2001:db8::1` or `fd00::1` or `::1`
- **THEN** exactly one IpV6 span is detected covering the address

#### Scenario: Full 8-group IPv6 still detected

- **WHEN** the user sends `2001:0db8:85a3:0000:0000:8a2e:0370:7334`
- **THEN** exactly one IpV6 span is detected
