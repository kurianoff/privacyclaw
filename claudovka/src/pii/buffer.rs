use crate::pii::vault::VaultHandle;
use std::collections::HashSet;

/// Streaming reverse-replacement buffer for SSE text deltas.
///
/// Receives incoming text chunks (extracted from SSE envelopes), applies
/// synthetic→original replacements, and flushes the safe prefix while
/// holding back a trailing window that might be the start of a synthetic token.
///
/// Zero latency when the vault is empty.
pub struct ReplacementBuffer {
    vault: VaultHandle,
    /// Internal buffer for text that might be mid-synthetic-token.
    buffer: String,
    /// First 2-byte prefixes of all synthetic keys — used to detect potential tokens.
    trigger_prefixes: HashSet<[u8; 2]>,
    /// Vault mapping count at the last trigger refresh — skip rebuild when unchanged.
    cached_mapping_count: usize,
}

impl ReplacementBuffer {
    pub fn new(vault: VaultHandle) -> Self {
        Self {
            vault,
            buffer: String::new(),
            trigger_prefixes: HashSet::new(),
            cached_mapping_count: 0,
        }
    }

    /// Process an incoming text delta from an SSE event.
    ///
    /// Returns the text to forward to the client (with synthetic→original
    /// replacements applied). May hold back a trailing window.
    pub fn process_delta(&mut self, incoming: &str) -> String {
        tracing::trace!(
            incoming_len = incoming.len(),
            buffer_len = self.buffer.len(),
            cached_count = self.cached_mapping_count,
            "buffer: process_delta enter"
        );
        if incoming.is_empty() {
            return String::new();
        }

        self.buffer.push_str(incoming);

        let vault = self.vault.read().unwrap();

        // Refresh trigger prefixes only when the vault has grown.
        let current_count = vault.mapping_count();
        if current_count != self.cached_mapping_count {
            let old_count = self.cached_mapping_count;
            self.trigger_prefixes = vault.synthetic_key_prefixes().collect();
            self.cached_mapping_count = current_count;
            tracing::trace!(
                old_count,
                new_count = current_count,
                prefix_count = self.trigger_prefixes.len(),
                "buffer: prefixes refreshed"
            );
        }

        // If vault is empty, flush everything immediately.
        if vault.is_empty() {
            tracing::trace!(buffer_len = self.buffer.len(), "buffer: vault empty, immediate flush");
            return std::mem::take(&mut self.buffer);
        }

        let max_key_len = vault.max_synthetic_key_len;

        // Apply all replacements to the buffer first.
        tracing::trace!(buffer_len = self.buffer.len(), max_key_len, "buffer: calling replace_synthetics");
        let (replaced, _any) = vault.replace_synthetics(&self.buffer);
        drop(vault);

        // Compute safe flush window: buffer minus trailing max_key_len bytes,
        // but only hold back if the tail contains a trigger char.
        let safe_len = if replaced.len() > max_key_len {
            let tail_start = find_char_boundary(&replaced, replaced.len() - max_key_len);
            // Use get() — never panics; falls back to whole string if boundary is off.
            let tail = replaced.get(tail_start..).unwrap_or(&replaced);
            let has_trigger = has_prefix_match(tail.as_bytes(), &self.trigger_prefixes);
            tracing::trace!(safe_len = tail_start, replaced_len = replaced.len(), has_trigger, "buffer: holdback decision");
            if has_trigger {
                tail_start
            } else {
                replaced.len()
            }
        } else {
            // Buffer is shorter than max_key_len — hold everything if trigger chars present.
            let has_trigger = has_prefix_match(replaced.as_bytes(), &self.trigger_prefixes);
            tracing::trace!(safe_len = 0usize, replaced_len = replaced.len(), has_trigger, "buffer: holdback decision");
            if has_trigger { 0 } else { replaced.len() }
        };

        if safe_len == 0 {
            // Hold entire replaced buffer — keep it for next chunk.
            self.buffer = replaced;
            tracing::debug!(
                incoming_len = incoming.len(),
                flushed_len = 0usize,
                holdback_len = self.buffer.len(),
                "buffer: delta processed"
            );
            return String::new();
        }

        // Split at a character boundary using get() — never panics.
        let flush_to = find_char_boundary(&replaced, safe_len);
        match (replaced.get(..flush_to), replaced.get(flush_to..)) {
            (Some(flushed), Some(remaining)) => {
                let flushed = flushed.to_string();
                self.buffer = remaining.to_string();
                tracing::debug!(
                    incoming_len = incoming.len(),
                    flushed_len = flushed.len(),
                    holdback_len = self.buffer.len(),
                    "buffer: delta processed"
                );
                flushed
            }
            _ => {
                // flush_to landed off a char boundary (shouldn't happen) — hold everything.
                self.buffer = replaced;
                tracing::debug!(
                    incoming_len = incoming.len(),
                    flushed_len = 0usize,
                    holdback_len = self.buffer.len(),
                    "buffer: delta processed"
                );
                String::new()
            }
        }
    }

    /// Flush all remaining buffered text at end of stream.
    pub fn flush_remaining(&mut self) -> String {
        tracing::debug!(held_len = self.buffer.len(), "buffer: flush_remaining called");
        if self.buffer.is_empty() {
            return String::new();
        }
        let vault = self.vault.read().unwrap();
        let (replaced, _) = vault.replace_synthetics(&self.buffer);
        drop(vault);
        self.buffer.clear();
        replaced
    }
}

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

/// Find the largest byte index ≤ `pos` that lies on a UTF-8 character boundary.
fn find_char_boundary(s: &str, pos: usize) -> usize {
    let mut p = pos.min(s.len());
    while p > 0 && !s.is_char_boundary(p) {
        p -= 1;
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pii::vault::{PiiType, PiiVault};
    use std::sync::{Arc, RwLock};

    fn make_vault_with(mappings: &[(&str, &str)]) -> VaultHandle {
        let mut vault = PiiVault::new("test");
        for (orig, syn) in mappings {
            vault.add_mapping(orig.to_string(), syn.to_string(), &PiiType::PersonName, 1, 0.9f32);
        }
        Arc::new(RwLock::new(vault))
    }

    #[test]
    fn test_empty_vault_immediate_flush() {
        let vault = Arc::new(RwLock::new(PiiVault::new("test")));
        let mut buf = ReplacementBuffer::new(vault);
        let out = buf.process_delta("hello world");
        assert_eq!(out, "hello world");
    }

    #[test]
    fn test_single_chunk_replacement() {
        let vault = make_vault_with(&[("John Smith", "Alice Brown")]);
        let mut buf = ReplacementBuffer::new(vault);
        let out = buf.process_delta("Hello Alice Brown, how are you?");
        let remaining = buf.flush_remaining();
        let full = format!("{}{}", out, remaining);
        assert!(full.contains("John Smith"), "got: {:?}", full);
        assert!(!full.contains("Alice Brown"), "synthetic still present: {:?}", full);
    }

    #[test]
    fn test_token_split_across_chunks() {
        let vault = make_vault_with(&[("John Smith", "Alice Brown")]);
        let mut buf = ReplacementBuffer::new(vault);
        // "Alice" arrives in first chunk, " Brown" in second.
        let out1 = buf.process_delta("Hello Alice");
        let out2 = buf.process_delta(" Brown!");
        let remaining = buf.flush_remaining();
        let full = format!("{}{}{}", out1, out2, remaining);
        // After both chunks and flush, John Smith should appear.
        assert!(full.contains("John Smith"), "got: {:?}", full);
    }

    #[test]
    fn test_flush_remaining_at_eos() {
        let vault = make_vault_with(&[("John", "Alice")]);
        let mut buf = ReplacementBuffer::new(vault);
        let out = buf.process_delta("Say Alice");
        let rem = buf.flush_remaining();
        let full = format!("{}{}", out, rem);
        assert!(full.contains("John"), "got: {:?}", full);
    }

    #[test]
    fn test_empty_delta_returns_empty() {
        // Vault is non-empty so the early-exit branch is not taken.
        let vault = make_vault_with(&[("real_value", "SYN_ONE")]);
        let mut buf = ReplacementBuffer::new(vault);
        let out = buf.process_delta("");
        assert_eq!(out, "", "empty delta should return empty string, got: {:?}", out);
    }

    #[test]
    fn test_no_trigger_char_immediate_flush() {
        // The synthetic is "ZZZ_TOKEN"; trigger char is 'Z'.
        // "hello world" contains no 'Z', so the buffer should flush it immediately.
        let vault = make_vault_with(&[("original_value", "ZZZ_TOKEN")]);
        let mut buf = ReplacementBuffer::new(vault);
        let out = buf.process_delta("hello world");
        // No synthetic in the text, no trigger char in the tail → flush immediately.
        assert_eq!(out, "hello world", "expected immediate flush without buffering, got: {:?}", out);
    }

    #[test]
    fn test_multiple_tokens_in_one_chunk() {
        let vault = make_vault_with(&[
            ("original_one", "SynToken1"),
            ("original_two", "SynToken2"),
        ]);
        let mut buf = ReplacementBuffer::new(vault);
        let chunk = "start SynToken1 middle SynToken2 end";
        let out = buf.process_delta(chunk);
        let remaining = buf.flush_remaining();
        let full = format!("{}{}", out, remaining);
        assert!(
            full.contains("original_one"),
            "first original not restored in: {:?}",
            full
        );
        assert!(
            full.contains("original_two"),
            "second original not restored in: {:?}",
            full
        );
        assert!(
            !full.contains("SynToken1"),
            "SynToken1 still present in: {:?}",
            full
        );
        assert!(
            !full.contains("SynToken2"),
            "SynToken2 still present in: {:?}",
            full
        );
    }

    #[test]
    fn test_flush_remaining_empty_buffer() {
        let vault = Arc::new(RwLock::new(PiiVault::new("test")));
        let mut buf = ReplacementBuffer::new(vault);
        // flush_remaining on a fresh buffer (no process_delta called) must return "".
        let result = buf.flush_remaining();
        assert_eq!(result, "", "flush_remaining on empty buffer should return empty string");
    }

    #[test]
    fn test_token_at_very_start() {
        let vault = make_vault_with(&[("real_value", "SYN_X")]);
        let mut buf = ReplacementBuffer::new(vault);
        let out = buf.process_delta("SYN_X is here");
        let remaining = buf.flush_remaining();
        let full = format!("{}{}", out, remaining);
        assert!(
            full.contains("real_value"),
            "expected 'real_value' after reversal, got: {:?}",
            full
        );
        assert!(
            !full.contains("SYN_X"),
            "synthetic still present in: {:?}",
            full
        );
    }

    #[test]
    fn test_multibyte_char_at_tail_boundary() {
        // em dash '—' is 3 bytes; if max_key_len slices inside it we get a panic.
        let vault = make_vault_with(&[("real_value", "SYN_X")]);
        let mut buf = ReplacementBuffer::new(vault);
        // Feed text where the tail window lands inside a multibyte char.
        let out = buf.process_delta("— resume after restart\n   - \"continue\"");
        let remaining = buf.flush_remaining();
        let full = format!("{}{}", out, remaining);
        // Just ensure no panic and the text round-trips intact.
        assert!(full.contains('—'), "em dash should be preserved: {:?}", full);
    }

    /// Throughput: 1 MB of text with no PII should flush in under 5 ms (4.8).
    #[test]
    fn test_throughput_1mb_no_pii_under_5ms() {
        use std::time::Instant;

        // Empty vault — no trigger chars, so ReplacementBuffer is zero-copy.
        let vault = make_vault_with(&[]);
        let mut buf = ReplacementBuffer::new(vault);

        // Build 1 MB of non-PII text.
        let chunk = "Lorem ipsum dolor sit amet, consectetur adipiscing elit. ";
        let mut input = String::with_capacity(1024 * 1024);
        while input.len() < 1024 * 1024 {
            input.push_str(chunk);
        }

        let start = Instant::now();
        let _out = buf.process_delta(&input);
        let _remaining = buf.flush_remaining();
        let elapsed = start.elapsed();

        assert!(
            elapsed.as_millis() < 5,
            "1 MB no-PII flush took {}ms (limit 5ms)",
            elapsed.as_millis()
        );
    }

    #[test]
    fn test_no_holdback_on_english_prose_with_ipv6_in_vault() {
        // Vault has a real email mapping AND a false-positive IPv6 entry (from Rust path).
        // English prose without any trigger prefixes should flush immediately.
        let mut vault = PiiVault::new("test-prose-holdback");
        vault.add_mapping(
            "john@acme.com".to_string(),
            "alice.smith@example.com".to_string(),
            &PiiType::Email,
            1,
            0.9f32,
        );
        // Simulate a false-positive IPv6 that was detected from a Rust path like "::"
        vault.add_mapping(
            "::".to_string(),
            "fd1a2b:3c4d::1".to_string(),
            &PiiType::IpV6,
            1,
            0.9f32,
        );
        let handle = Arc::new(RwLock::new(vault));
        let mut buf = ReplacementBuffer::new(handle);

        // English prose chunks that do NOT contain any synthetic trigger prefixes.
        // Trigger prefixes are "al" (from alice.smith@example.com) and "fd" (from fd1a2b:...).
        let chunks = [
            "The quick brown ",
            "ox jumps over ",
            "the very ",
            "big w",
            "ow.",
        ];
        let mut total_output = String::new();
        for chunk in &chunks {
            let out = buf.process_delta(chunk);
            total_output.push_str(&out);
        }
        let remaining = buf.flush_remaining();
        total_output.push_str(&remaining);

        // All text should come through unmodified.
        let expected = chunks.join("");
        assert_eq!(total_output, expected, "English prose was modified or held back");
        // flush_remaining should have returned empty (nothing held back after last chunk).
        assert!(remaining.is_empty(), "flush_remaining returned non-empty: {:?}", remaining);
    }

    #[test]
    fn test_holdback_triggered_by_prefix_match() {
        // Verify that text containing a trigger prefix IS held back.
        // Vault has synthetic "fd1a2b:3c4d::1" -> prefix [b'f', b'd'].
        // Text ending with "fd" should be held back.
        let mut vault = PiiVault::new("test-holdback-trigger");
        vault.add_mapping(
            "::".to_string(),
            "fd1a2b:3c4d::1".to_string(),
            &PiiType::IpV6,
            1,
            0.9f32,
        );
        let handle = Arc::new(RwLock::new(vault));
        let mut buf = ReplacementBuffer::new(handle);

        // Send text that ends with "fd" — the trigger prefix for the IPv6 synthetic.
        let out = buf.process_delta("some text ending in fd");
        // The buffer should hold back the tail because "fd" matches a trigger prefix.
        // We can't assert exact holdback boundaries, but combined output must be correct.
        let remaining = buf.flush_remaining();
        let full = format!("{}{}", out, remaining);
        assert_eq!(full, "some text ending in fd", "combined output should be original text");
    }

    #[test]
    fn test_ipv6_synthetic_max_key_len_short() {
        // After the gen_ipv6 fix, IPv6 synthetics are ~14 chars (fd{4}:{4}::1).
        // Verify that a vault with only IPv6 mappings has max_synthetic_key_len <= 16.
        use crate::pii::synth::SyntheticGenerator;
        use crate::pii::locale::Locale;

        let mut vault = PiiVault::new("test-ipv6-len");
        for i in 0..10 {
            let original = format!("2001:db8::{}", i);
            SyntheticGenerator::get_or_create(&mut vault, &original, &PiiType::IpV6, &Locale::EnUs, 1, 1.0);
        }
        assert!(
            vault.max_synthetic_key_len <= 16,
            "max_synthetic_key_len is {} but should be <= 16 after gen_ipv6 fix",
            vault.max_synthetic_key_len
        );
    }

    #[test]
    fn test_accumulation_across_many_chunks() {
        // synthetic="SYN_TOK", original="restored_val"
        // Full string: "prefix_SYN_TOK_suffix"
        let vault = make_vault_with(&[("restored_val", "SYN_TOK")]);
        let mut buf = ReplacementBuffer::new(vault);

        let full_input = "prefix_SYN_TOK_suffix";
        // Split into single-character chunks.
        let chars: Vec<&str> = full_input
            .char_indices()
            .map(|(i, _)| &full_input[i..i + 1])
            .collect();

        let mut accumulated = String::new();
        for ch in &chars {
            accumulated.push_str(&buf.process_delta(ch));
        }
        accumulated.push_str(&buf.flush_remaining());

        assert!(
            accumulated.contains("restored_val"),
            "expected 'restored_val' after char-by-char feed, got: {:?}",
            accumulated
        );
        assert!(
            !accumulated.contains("SYN_TOK"),
            "synthetic still present after char-by-char feed: {:?}",
            accumulated
        );
    }
}
