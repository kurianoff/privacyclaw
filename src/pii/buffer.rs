use crate::pii::vault::VaultHandle;
use std::collections::HashSet;

// Byte sequence that triggers XML-token holdback.
const XML_TOKEN_OPEN: &[u8] = b"<pii";
const XML_TOKEN_CLOSE: &[u8] = b"</pii>";

/// Streaming reverse-replacement buffer for SSE text deltas.
///
/// Receives incoming text chunks (extracted from SSE envelopes), applies
/// synthetic→original replacements, and flushes the safe prefix while
/// holding back a trailing window that might be the start of a synthetic token.
///
/// Two holdback paths:
///   - XML-token path: when `<pii` is found, hold until `</pii>` is complete,
///     then apply the cascade matcher (Levels 1–4) before flushing.
///   - Display-value path (Level 5): Aho-Corasick over bare display values,
///     triggered by 2-byte prefix matching.
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
            // Only include display-value prefixes, never XML-token prefixes (task 6.4).
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

        // ── XML-token holdback (task 6.1): scan for `<pii` in the buffer.
        // If a complete `<pii ...>...</pii>` token is present, apply the cascade
        // matcher before continuing. If only a partial token is present, hold back
        // up to the `<pii` start position so we don't flush a split token.
        let xml_processed = apply_xml_token_cascade(&self.buffer, &vault);
        drop(vault);

        // Compute safe flush window: buffer minus trailing max_key_len bytes,
        // but only hold back if the tail contains a trigger char.
        let safe_len = compute_safe_flush_len(&xml_processed, max_key_len, &self.trigger_prefixes);

        if safe_len == 0 {
            // Hold entire replaced buffer — keep it for next chunk.
            self.buffer = xml_processed;
            tracing::debug!(
                incoming_len = incoming.len(),
                flushed_len = 0usize,
                holdback_len = self.buffer.len(),
                "buffer: delta processed"
            );
            return String::new();
        }

        // Split at a character boundary using get() — never panics.
        let flush_to = find_char_boundary(&xml_processed, safe_len);
        match (xml_processed.get(..flush_to), xml_processed.get(flush_to..)) {
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
                self.buffer = xml_processed;
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
        // Apply XML-token cascade (includes Level-5 replacement for non-XML segments).
        let replaced = apply_xml_token_cascade(&self.buffer, &vault);
        drop(vault);
        self.buffer.clear();
        replaced
    }
}

// ── XML-token cascade (tasks 6.1–6.4) ────────────────────────────────────────

/// Apply the XML-token cascade matcher to `text`.
///
/// Scans for complete `<pii id="...">...</pii>` tokens and reverses each one
/// to its original PII value using a four-level cascade:
///   L1: exact full-token lookup in `vault.full_token_to_original`
///   L2: extract token_id, call `vault.get_by_token_id`
///   L3: extract display value, call `vault.get_by_display_value`
///   L4: no match — log WARN and pass token through unchanged (Part II stub)
///
/// Partial `<pii` sequences that have no matching `</pii>` are left in place
/// so the caller can hold them back for the next chunk.
///
/// Level-5 Aho-Corasick over display values is applied separately in
/// `flush_remaining` / the standard `replace_synthetics` call; it is NOT
/// applied here to avoid double-processing.
fn apply_xml_token_cascade(text: &str, vault: &crate::pii::vault::PiiVault) -> String {
    // Fast path: no `<pii` at all.
    if !text.as_bytes().windows(XML_TOKEN_OPEN.len()).any(|w| w == XML_TOKEN_OPEN) {
        // No XML tokens present — apply standard Level-5 replacement and return.
        let (replaced, _) = vault.replace_synthetics(text);
        return replaced;
    }

    let mut result = String::with_capacity(text.len());
    let mut pos = 0usize;

    while pos < text.len() {
        // Find next `<pii` from current position.
        let open_pos = match find_subsequence(text.as_bytes(), pos, XML_TOKEN_OPEN) {
            Some(p) => p,
            None => {
                // No more XML tokens — flush remainder via Level-5 replacement.
                let tail = &text[pos..];
                let (replaced_tail, _) = vault.replace_synthetics(tail);
                result.push_str(&replaced_tail);
                break;
            }
        };

        // Flush text before this token via Level-5 replacement.
        if open_pos > pos {
            let prefix = &text[pos..open_pos];
            let (replaced_prefix, _) = vault.replace_synthetics(prefix);
            result.push_str(&replaced_prefix);
        }

        // Find the matching `</pii>` close tag.
        let close_pos = match find_subsequence(text.as_bytes(), open_pos, XML_TOKEN_CLOSE) {
            Some(p) => p,
            None => {
                // Incomplete token — leave from `open_pos` onwards in buffer (hold back).
                result.push_str(&text[open_pos..]);
                break;
            }
        };

        let token_end = close_pos + XML_TOKEN_CLOSE.len();
        let full_token = &text[open_pos..token_end];

        tracing::debug!(
            token_len = full_token.len(),
            "buffer: cascade: found complete XML token"
        );

        // ── Level 1: exact full-token lookup ──────────────────────────────────
        if let Some(original) = vault.full_token_to_original.get(full_token) {
            tracing::debug!(level = 1, "buffer: cascade: L1 hit");
            result.push_str(original);
            pos = token_end;
            continue;
        }

        // ── Level 2: extract id="TOKEN_ID", lookup by token_id ────────────────
        if let Some(token_id) = extract_xml_attr(full_token, "id") {
            if let Some(original) = vault.get_by_token_id(&token_id) {
                tracing::debug!(level = 2, "buffer: cascade: L2 hit");
                result.push_str(original);
                pos = token_end;
                continue;
            }

            // ── Level 3: extract inner text (display value), lookup ────────────
            if let Some(display_value) = extract_xml_inner(full_token) {
                if let Some(original) = vault.get_by_display_value(&display_value) {
                    tracing::debug!(level = 3, "buffer: cascade: L3 hit");
                    result.push_str(original);
                    pos = token_end;
                    continue;
                }
            }
        }

        // ── Level 4: no match — stub (Part II) ────────────────────────────────
        tracing::warn!(
            token = full_token,
            "buffer: cascade Level 4 (hypothesis match) not yet implemented; passing token through"
        );
        result.push_str(full_token);
        pos = token_end;
    }

    result
}

/// Find the byte offset of `needle` in `haystack[start..]`, returning an absolute offset.
fn find_subsequence(haystack: &[u8], start: usize, needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < start + needle.len() {
        return None;
    }
    haystack[start..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| start + p)
}

/// Extract an XML attribute value: `attr="value"` from a string like `<pii id="abc123">...`.
/// Returns the value without quotes.
fn extract_xml_attr<'a>(token: &'a str, attr: &str) -> Option<String> {
    let search = format!("{}=\"", attr);
    let start = token.find(&search)? + search.len();
    let end = token[start..].find('"')?;
    Some(token[start..start + end].to_string())
}

/// Extract the inner text content from `<pii id="...">INNER</pii>`.
fn extract_xml_inner(token: &str) -> Option<String> {
    // Find the end of the opening tag `>`.
    let open_end = token.find('>')?;
    let inner_start = open_end + 1;
    // Find the start of the closing tag `</pii>`.
    let close_start = token.rfind("</")?;
    if close_start < inner_start {
        return None;
    }
    Some(token[inner_start..close_start].to_string())
}

/// Compute how many bytes from the front of `text` are safe to flush.
///
/// Holds back a trailing window of `max_key_len` bytes (adjusted to a char
/// boundary) whenever that window contains a synthetic-token trigger prefix.
/// Also holds back from any partial `<pii` sequence found in the tail that
/// has no matching `</pii>` close tag — so split XML tokens are never flushed.
///
/// Returns `text.len()` when nothing needs to be held back, or `0` when the
/// entire buffer is shorter than `max_key_len` and contains a trigger.
fn compute_safe_flush_len(text: &str, max_key_len: usize, prefixes: &HashSet<[u8; 2]>) -> usize {
    // Check for a partial `<pii` in the text that has no matching `</pii>`.
    // If found, hold back from the start of that partial open tag.
    if let Some(xml_holdback) = xml_token_holdback_pos(text) {
        tracing::trace!(safe_len = xml_holdback, replaced_len = text.len(), xml_holdback = true, "buffer: holdback decision (xml partial)");
        return xml_holdback;
    }

    if text.len() > max_key_len {
        let tail_start = find_char_boundary(text, text.len() - max_key_len);
        let tail = text.get(tail_start..).unwrap_or(text);
        let has_trigger = has_prefix_match(tail.as_bytes(), prefixes);
        tracing::trace!(safe_len = tail_start, replaced_len = text.len(), has_trigger, "buffer: holdback decision");
        if has_trigger { tail_start } else { text.len() }
    } else {
        // Buffer is shorter than max_key_len — hold everything if trigger present.
        let has_trigger = has_prefix_match(text.as_bytes(), prefixes);
        tracing::trace!(safe_len = 0usize, replaced_len = text.len(), has_trigger, "buffer: holdback decision");
        if has_trigger { 0 } else { text.len() }
    }
}

/// If `text` contains a `<pii` sequence that has no matching `</pii>` close tag,
/// return the byte offset of the last such partial open tag (safe flush boundary).
/// Returns `None` if no partial XML token is present.
fn xml_token_holdback_pos(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    // Find the last occurrence of `<pii` in the text.
    let last_open = bytes
        .windows(XML_TOKEN_OPEN.len())
        .enumerate()
        .filter(|(_, w)| *w == XML_TOKEN_OPEN)
        .map(|(i, _)| i)
        .last()?;

    // Check if there is a `</pii>` after this `<pii`.
    let has_close = find_subsequence(bytes, last_open, XML_TOKEN_CLOSE).is_some();
    if has_close {
        // Complete token — no holdback needed for this one.
        None
    } else {
        // Partial token: hold back from `last_open`.
        Some(last_open)
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

    // ── Group 6: XML-token cascade tests (tasks 6.1–6.5) ─────────────────────

    fn make_vault_with_token_id(
        original: &str,
        display_value: &str,
        token_id: &str,
    ) -> VaultHandle {
        let mut vault = PiiVault::new("test-cascade");
        vault.add_mapping_with_token_id(
            original,
            display_value,
            token_id,
            &PiiType::Email,
            3,
            1.0,
        );
        Arc::new(RwLock::new(vault))
    }

    /// L1: exact full-token match reverses to original.
    #[test]
    fn buffer_xml_token_reversed_level1() {
        let vault = make_vault_with_token_id("alice@acme.com", "synth@example.com", "a3f9b2c1");
        let mut buf = ReplacementBuffer::new(vault);
        let token = r#"<pii id="a3f9b2c1">synth@example.com</pii>"#;
        let out = buf.process_delta(token);
        let remaining = buf.flush_remaining();
        let full = format!("{out}{remaining}");
        assert_eq!(full, "alice@acme.com", "L1: expected original, got: {full:?}");
    }

    /// L2: token_id matches but full token string differs — still reverses to original.
    #[test]
    fn buffer_xml_token_reversed_level2() {
        // Insert mapping so only token_id_to_original is populated (not full_token).
        // We simulate this by inserting a different display_value in the full token.
        let original = "bob@corp.com";
        let display = "synth_bob@example.com";
        let tid = "b1c2d3e4";
        let vault = make_vault_with_token_id(original, display, tid);
        let _buf = ReplacementBuffer::new(vault);
        // Use the correct token_id but a display value that won't match L1 exactly
        // because we provide the *correct* XML token here — L1 will hit.
        // To specifically test L2, we need to delete the L1 entry.
        // Strategy: build vault manually with only token_id_to_original populated.
        let mut v2 = PiiVault::new("test-l2");
        v2.add_mapping_with_token_id(original, display, tid, &PiiType::Email, 3, 1.0);
        // Remove from full_token_to_original to force L2 path.
        let full_key = format!(r#"<pii id="{tid}">{display}</pii>"#);
        v2.full_token_to_original.remove(&full_key);
        let handle = Arc::new(RwLock::new(v2));
        let mut buf2 = ReplacementBuffer::new(handle);
        let token = format!(r#"<pii id="{tid}">{display}</pii>"#);
        let out = buf2.process_delta(&token);
        let remaining = buf2.flush_remaining();
        let full = format!("{out}{remaining}");
        assert_eq!(full, original, "L2: expected original, got: {full:?}");
    }

    /// L3: display value matches (no token_id match) — reverses to original.
    #[test]
    fn buffer_xml_token_reversed_level3() {
        let original = "carol@corp.com";
        let display = "synth_carol@example.com";
        let tid = "c1d2e3f4";
        let mut v = PiiVault::new("test-l3");
        v.add_mapping_with_token_id(original, display, tid, &PiiType::Email, 3, 1.0);
        // Remove L1 and L2 entries to force L3 path.
        let full_key = format!(r#"<pii id="{tid}">{display}</pii>"#);
        v.full_token_to_original.remove(&full_key);
        v.token_id_to_original.remove(tid);
        let handle = Arc::new(RwLock::new(v));
        let mut buf = ReplacementBuffer::new(handle);
        let token = format!(r#"<pii id="{tid}">{display}</pii>"#);
        let out = buf.process_delta(&token);
        let remaining = buf.flush_remaining();
        let full = format!("{out}{remaining}");
        assert_eq!(full, original, "L3: expected original, got: {full:?}");
    }

    /// L4: no match at any level — token passed through unchanged, WARN logged.
    #[test]
    fn buffer_xml_token_passthrough_level4() {
        // Vault has no entries matching this token.
        let vault = Arc::new(RwLock::new(PiiVault::new("test-l4")));
        let mut buf = ReplacementBuffer::new(vault);
        let token = r#"<pii id="xxxxxxxx">unknown@example.com</pii>"#;
        let out = buf.process_delta(token);
        let remaining = buf.flush_remaining();
        let full = format!("{out}{remaining}");
        // Token is passed through unchanged (Level 4 stub).
        assert_eq!(full, token, "L4: token must pass through unchanged, got: {full:?}");
    }

    /// Split: `<pii` arrives in one chunk, `</pii>` in the next — must still reverse.
    #[test]
    fn buffer_xml_token_split_across_chunks() {
        let vault = make_vault_with_token_id("dave@acme.com", "synth_dave@example.com", "d1e2f3a4");
        let mut buf = ReplacementBuffer::new(vault);
        // Split the token across two chunks.
        let token = r#"<pii id="d1e2f3a4">synth_dave@example.com</pii>"#;
        let mid = token.len() / 2;
        let chunk1 = &token[..mid];
        let chunk2 = &token[mid..];
        let out1 = buf.process_delta(chunk1);
        let out2 = buf.process_delta(chunk2);
        let remaining = buf.flush_remaining();
        let full = format!("{out1}{out2}{remaining}");
        assert_eq!(full, "dave@acme.com", "split token: expected original, got: {full:?}");
    }

    /// Trigger prefixes must NOT contain `[b'<', b'p']` after vault insert.
    #[test]
    fn buffer_trigger_prefixes_no_xml_prefix() {
        let vault = make_vault_with_token_id("eve@acme.com", "synth_eve@example.com", "e1f2a3b4");
        let v = vault.read().unwrap();
        // Ensure synthetic_key_prefixes does not include ['<', 'p'].
        let prefixes: HashSet<[u8; 2]> = v.synthetic_key_prefixes().collect();
        assert!(
            !prefixes.contains(&[b'<', b'p']),
            "trigger prefixes must not include [b'<', b'p'], got: {prefixes:?}"
        );
    }
}
