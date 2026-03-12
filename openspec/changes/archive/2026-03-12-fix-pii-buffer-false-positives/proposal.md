# Change Proposal: fix-pii-buffer-false-positives

## Status
Draft

## Problem

Inbound PII unmasking is broken. When a user sends a message containing Rust source code (e.g., `crate::pii::vault::PiiVault`), the Tier 1 IPv6 regex falsely detects path separators (`::`) flanked by hex-valid identifier segments (`pii`, `ca`, `ce`, `fd`) as IPv6 addresses. This pollutes the `PiiVault` with false-positive entries carrying ~39-character synthetic IPv6 keys.

The `ReplacementBuffer` uses first characters of all synthetic keys as `trigger_chars` to decide when to hold back streaming text. With the false-positive IPv6 synthetics, `trigger_chars` becomes `{'f','d','a','b','c','e',':'}` — nearly all lowercase English letters. Because SSE streaming deltas are typically 5–20 characters (well below `max_synthetic_key_len = 39`), `process_delta` returns an empty string for virtually every call. The entire LLM response accumulates in the buffer and is delivered as a single burst at `content_block_stop`, rather than streaming through with replacements applied incrementally.

## Root Cause Chain

1. `src/pii/tier1.rs:97` — IPv6 regex second alternation `(?:[0-9a-fA-F]{1,4}:)*::[0-9a-fA-F:]*` is unbounded and matches Rust path separators.
2. `src/pii/synth.rs:115-119` — `gen_ipv6()` produces 7-group synthetics (~39 chars), inflating `max_synthetic_key_len`.
3. `src/pii/vault.rs` — `synthetic_key_first_chars()` includes all vault entries; no filtering for false positives.
4. `src/pii/buffer.rs:76-78` — Single-char trigger check: if any char in the buffer tail matches a trigger char, hold everything. With hex chars in trigger set, nearly all English text triggers holdback.

## Solution

Four targeted changes:

1. **Tighten the IPv6 regex** — add word-boundary negative lookbehind and a post-match `ipv6_valid()` validator that rejects any match where a segment exceeds 4 chars (Rust identifiers like `vault`, `buffer`, `pii` are >4 chars) or fewer than 2 colons are present.

2. **Shorten IPv6 synthetics** — reduce `gen_ipv6()` from 7 random groups to 2: `fd{g1}:{g2}::1` (~15 chars vs ~39 chars). This shrinks `max_synthetic_key_len`, reducing the holdback window.

3. **Expose 2-byte key prefixes from vault** — add `synthetic_key_prefixes() -> impl Iterator<Item = [u8; 2]>` to `PiiVault`. First 2 bytes of a synthetic are far more specific than a single char.

4. **2-byte trigger prefix matching in buffer** — replace `trigger_chars: HashSet<char>` with `trigger_prefixes: HashSet<[u8; 2]>`. The buffer only holds back when the tail contains a 2-byte substring that matches a known synthetic prefix. English prose almost never starts words with `fd`, `fe80`, or `10.` — the false trigger rate drops dramatically.

## Scope

**In scope:**
- `src/pii/tier1.rs` — IPv6 regex + `ipv6_valid()` validator
- `src/pii/synth.rs` — `gen_ipv6()` 2-group format
- `src/pii/vault.rs` — `synthetic_key_prefixes()` method
- `src/pii/buffer.rs` — `trigger_prefixes` field + `refresh_triggers()` + `has_prefix_match()` helper

**Out of scope (tracked separately):**
- Excluding private IPv4 ranges (private IPs remain legitimate PII)
- Detection gaps in Anthropic `system` field and `tool_use.input`
- LLM-mutated-synthetic handling (case-insensitive Aho-Corasick)

## Acceptance Criteria

1. `Tier1Detector::detect("use crate::pii::vault::PiiVault;", Locale::EnUs)` returns zero `IpV6` spans.
2. `Tier1Detector::detect("addr: 2001:db8::1", Locale::EnUs)` returns one `IpV6` span.
3. `Tier1Detector::detect("addr: fe80::1", Locale::EnUs)` returns one `IpV6` span.
4. `gen_ipv6()` produces strings of length ≤ 16 chars.
5. `ReplacementBuffer` with a vault containing one email + one IPv6 false-positive flushes streaming text incrementally (not all at once).
6. All existing `cargo test` tests continue to pass.
7. `cargo clippy -- -D warnings` passes.
