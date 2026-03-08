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
    /// First characters of all synthetic keys — used to detect potential tokens.
    trigger_chars: HashSet<char>,
}

impl ReplacementBuffer {
    pub fn new(vault: VaultHandle) -> Self {
        Self {
            vault,
            buffer: String::new(),
            trigger_chars: HashSet::new(),
        }
    }

    /// Update the trigger chars from the current vault state.
    ///
    /// Call after the vault has been written to.
    fn refresh_trigger_chars(&mut self) {
        let vault = self.vault.read().unwrap();
        self.trigger_chars = vault.synthetic_key_first_chars().collect();
    }

    /// Process an incoming text delta from an SSE event.
    ///
    /// Returns the text to forward to the client (with synthetic→original
    /// replacements applied). May hold back a trailing window.
    pub fn process_delta(&mut self, incoming: &str) -> String {
        if incoming.is_empty() {
            return String::new();
        }

        // Refresh trigger chars (vault may have grown).
        self.refresh_trigger_chars();

        self.buffer.push_str(incoming);

        let vault = self.vault.read().unwrap();

        // If vault is empty, flush everything immediately.
        if vault.is_empty() {
            return std::mem::take(&mut self.buffer);
        }

        let max_key_len = vault.max_synthetic_key_len;

        // Apply all replacements to the buffer first.
        let (replaced, _any) = vault.replace_synthetics(&self.buffer);
        drop(vault);

        // Compute safe flush window: buffer minus trailing max_key_len bytes,
        // but only hold back if the tail contains a trigger char.
        let safe_len = if replaced.len() > max_key_len {
            let tail = &replaced[replaced.len() - max_key_len..];
            let has_trigger = tail.chars().any(|c| self.trigger_chars.contains(&c));
            if has_trigger {
                replaced.len() - max_key_len
            } else {
                replaced.len()
            }
        } else {
            // Buffer is shorter than max_key_len — hold everything if trigger chars present.
            let has_trigger = replaced.chars().any(|c| self.trigger_chars.contains(&c));
            if has_trigger { 0 } else { replaced.len() }
        };

        if safe_len == 0 {
            // Hold entire replaced buffer — keep it for next chunk.
            self.buffer = replaced;
            return String::new();
        }

        // Split at a character boundary.
        let flush_to = find_char_boundary(&replaced, safe_len);
        let flushed = replaced[..flush_to].to_string();
        self.buffer = replaced[flush_to..].to_string();
        flushed
    }

    /// Flush all remaining buffered text at end of stream.
    pub fn flush_remaining(&mut self) -> String {
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
            vault.add_mapping(orig.to_string(), syn.to_string(), &PiiType::PersonName);
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
}
