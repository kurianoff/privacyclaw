# Tasks: fix-pii-buffer-false-positives

## 1. Tighten IPv6 regex in `src/pii/tier1.rs`

### 1.1 Add `ipv6_valid()` validator function
Add a `fn ipv6_valid(s: &str) -> bool` that returns false if:
- Fewer than 2 colon characters in the match
- Any colon-separated segment is longer than 4 characters

### 1.2 Update the IPv6 `PatternSet` to use word boundaries and validator
Replace the IPv6 pattern with one that adds `(?<![:\w])` negative lookbehind and call `.with_validator(ipv6_valid)`.

### 1.3 Add regression tests for IPv6 false positives
- `test_ipv6_no_false_positive_rust_path`: `"use crate::pii::vault::PiiVault"` → no IpV6 spans
- `test_ipv6_no_false_positive_double_colon_only`: `"foo::bar"` → no IpV6 spans
- `test_ipv6_abbreviated_detected`: `fe80::1`, `::1`, `2001:db8::1`, `fd00::1` → each produces one IpV6 span

---

## 2. Shorten IPv6 synthetic format in `src/pii/synth.rs`

### 2.1 Rewrite `gen_ipv6()` to use 2 random groups
```rust
fn gen_ipv6(&mut self) -> String {
    let g1 = format!("{:04x}", self.rng.gen_range(0u16..=0xffff));
    let g2 = format!("{:04x}", self.rng.gen_range(0u16..=0xffff));
    format!("fd{}:{}::1", g1, g2)
}
```
Result: `fd1a2b:3c4d::1` (14 chars) instead of the old ~39-char form.

---

## 3. Add `synthetic_key_prefixes()` to `src/pii/vault.rs`

### 3.1 Add method to `PiiVault`
```rust
pub fn synthetic_key_prefixes(&self) -> impl Iterator<Item = [u8; 2]> + '_ {
    self.synthetic_keys.iter().filter_map(|s| {
        let b = s.as_bytes();
        if b.len() >= 2 { Some([b[0], b[1]]) } else { None }
    })
}
```
Place after the existing `synthetic_key_first_chars()` method.

---

## 4. Replace single-char triggers with 2-byte prefix matching in `src/pii/buffer.rs`

### 4.1 Update `ReplacementBuffer` struct
Replace `trigger_chars: HashSet<char>` → `trigger_prefixes: HashSet<[u8; 2]>`.
Update `new()` accordingly.

### 4.2 Replace `refresh_trigger_chars()` with `refresh_triggers()`
```rust
fn refresh_triggers(&mut self) {
    let vault = self.vault.read().unwrap();
    self.trigger_prefixes = vault.synthetic_key_prefixes().collect();
}
```

### 4.3 Add `has_prefix_match()` free function
```rust
fn has_prefix_match(bytes: &[u8], prefixes: &HashSet<[u8; 2]>) -> bool {
    for window in bytes.windows(2) {
        if prefixes.contains(&[window[0], window[1]]) {
            return true;
        }
    }
    if let Some(&last) = bytes.last() {
        if prefixes.iter().any(|p| p[0] == last) {
            return true;
        }
    }
    false
}
```

### 4.4 Update `process_delta()` trigger check
Replace both occurrences of:
```rust
let has_trigger = ..chars().any(|c| self.trigger_chars.contains(&c));
```
with:
```rust
let has_trigger = has_prefix_match(replaced.as_bytes(), &self.trigger_prefixes);
// (long-buffer branch: use tail.as_bytes())
```

### 4.5 Update call site: `refresh_trigger_chars()` → `refresh_triggers()`

### 4.6 Add unit test for 2-byte prefix behaviour
Test that `ReplacementBuffer` with a vault containing one email synthetic AND a false-positive IPv6 synthetic does NOT hold back plain English text between SSE deltas.

---

## 5. Verify and run

### 5.1 `cargo build` — must succeed with no errors
### 5.2 `cargo clippy -- -D warnings` — must produce no warnings
### 5.3 `cargo test` — all tests must pass
### 5.4 `cargo test -p privacyclaw -- pii` — run only PII-related tests, confirm all pass

---

## Dependencies

- Task 3 must complete before Task 4 (buffer uses `synthetic_key_prefixes()`).
- Tasks 1 and 2 are independent of each other and of Tasks 3–4.
- Task 5 runs after all code tasks complete.
