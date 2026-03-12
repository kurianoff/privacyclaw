## ADDED Requirements

### Requirement: Credential Redaction in Log Output
The `fmt_headers` helper SHALL replace the values of `Authorization` and `X-Api-Key` headers with a redaction marker before they appear in any log output. Header names SHALL be preserved.

#### Scenario: Authorization header value redacted
- **WHEN** `fmt_headers` processes a header block containing `Authorization: Bearer sk-abc123`
- **THEN** the output contains the `Authorization` key but not the value `sk-abc123`

#### Scenario: X-Api-Key header value redacted
- **WHEN** `fmt_headers` processes a header block containing `X-Api-Key: my-secret-key`
- **THEN** the output contains `X-Api-Key` but not `my-secret-key`

#### Scenario: Non-sensitive headers pass through unchanged
- **WHEN** `fmt_headers` processes `Content-Type: application/json`
- **THEN** the full header including value is present in the output

### Requirement: Raw Byte Dump Truncation
The `fmt_chunk_hex` helper SHALL truncate its output to at most 256 bytes of source data when producing hex dumps for log messages.

#### Scenario: Long input truncated
- **WHEN** `fmt_chunk_hex` receives a 1024-byte slice
- **THEN** the output represents at most 256 bytes of input

#### Scenario: Short input not truncated
- **WHEN** `fmt_chunk_hex` receives a 10-byte slice
- **THEN** the output represents all 10 bytes with no truncation

#### Scenario: Empty input produces no panic
- **WHEN** `fmt_chunk_hex` receives an empty slice
- **THEN** it returns a defined (possibly empty) string without panicking
