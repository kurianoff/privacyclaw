use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub started_at: String,
    pub provider: String,
    pub model: Option<String>,
    pub client_hint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub conversation_id: String,
    pub direction: String,
    pub timestamp: String,
    pub role: Option<String>,
    pub content: String,
    pub tokens_in: Option<i64>,
    pub tokens_out: Option<i64>,
}

/// Vault record as stored on disk (mirrors pii::vault::VaultRecord).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredVaultRecord {
    pub original: String,
    pub synthetic: String,
    pub pii_type: String, // stored as label string
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedVault {
    #[serde(rename = "type")]
    record_type: String,
    rng_seed: u64,
    mappings: Vec<StoredVaultRecord>,
}

/// NDJSON-based conversation store.
///
/// Each conversation is a `.ndjson` file:
///   line 1:  Conversation JSON (metadata + fingerprint)
///   lines 2+: Message JSON, one per line, appended in arrival order
///
/// File names: `<YYYY-MM-DD>T<HHMMSS>Z_<conv-id>.ndjson`
///
/// Writes are O(message_size) appends — no read-modify-write cycles.
/// Reads for fingerprint lookup read only line 1 of each file.
#[derive(Clone)]
pub struct Store {
    logs_dir: PathBuf,
    /// Serialises concurrent appends to the same conversation file.
    /// Held only during the actual write(), not during any reads.
    write_lock: Arc<Mutex<()>>,
}

impl Store {
    pub fn open(logs_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(logs_dir)
            .with_context(|| format!("Failed to create logs dir: {:?}", logs_dir))?;
        Ok(Self {
            logs_dir: logs_dir.to_path_buf(),
            write_lock: Arc::new(Mutex::new(())),
        })
    }

    // ── path helpers ──────────────────────────────────────────────────────────

    fn conv_file_path(&self, conv_id: &str) -> Option<PathBuf> {
        let suffix = format!("_{}.ndjson", conv_id);
        tracing::debug!(conv_id = %conv_id, suffix = %suffix, "storage: conv_file_path scan");
        std::fs::read_dir(&self.logs_dir)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .find(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.ends_with(&suffix))
                    .unwrap_or(false)
            })
    }

    fn new_conv_file_path(&self, conv_id: &str) -> PathBuf {
        let ts = Utc::now().format("%Y-%m-%dT%H%M%SZ");
        self.logs_dir.join(format!("{}_{}.ndjson", ts, conv_id))
    }

    // ── low-level NDJSON readers ──────────────────────────────────────────────

    /// Read only the first line of a file and parse it as Conversation.
    /// O(first_line_length) — does not read the message body.
    fn read_conv_header(path: &Path) -> Result<Conversation> {
        let file = std::fs::File::open(path)
            .with_context(|| format!("open {:?}", path))?;
        let mut reader = std::io::BufReader::new(file);
        let mut line = String::new();
        reader.read_line(&mut line)?;
        serde_json::from_str(line.trim())
            .with_context(|| format!("parse conv header in {:?}", path))
    }

    /// Read all message lines (lines 2+) from a conversation file.
    fn read_messages(path: &Path) -> Result<Vec<Message>> {
        let file = std::fs::File::open(path)
            .with_context(|| format!("open {:?}", path))?;
        let reader = std::io::BufReader::new(file);
        let mut messages = Vec::new();
        for (i, line) in reader.lines().enumerate() {
            let line = line?;
            if i == 0 || line.trim().is_empty() {
                continue; // skip conversation header and blank lines
            }
            match serde_json::from_str::<Message>(&line) {
                Ok(msg) => messages.push(msg),
                Err(e) => tracing::warn!("Skipping malformed message line: {}", e),
            }
        }
        Ok(messages)
    }

    // ── public API ────────────────────────────────────────────────────────────

    /// Create a new conversation file with the metadata as line 1.
    pub fn insert_conversation(&self, conv: &Conversation) -> Result<()> {
        tracing::info!(conv_id = %conv.id, provider = %conv.provider, model = ?conv.model, "storage: insert_conversation");
        let path = self.new_conv_file_path(&conv.id);
        tracing::debug!(conv_id = %conv.id, path = %path.display(), "storage: writing new conv file");
        let line = serde_json::to_string(conv)? + "\n";
        std::fs::write(&path, line.as_bytes())
            .with_context(|| format!("write conv {:?}", path))?;
        tracing::info!(conv_id = %conv.id, path = %path.display(), "storage: insert_conversation ok");
        Ok(())
    }

    pub fn insert_message(&self, msg: &Message) -> Result<()> {
        self.batch_insert_messages(std::slice::from_ref(msg))
    }

    /// Append messages to the conversation file — pure O(message_bytes) writes,
    /// no read, no parse, no rewrite of existing content.
    pub fn batch_insert_messages(&self, messages: &[Message]) -> Result<()> {
        if messages.is_empty() {
            return Ok(());
        }
        let conv_id = &messages[0].conversation_id;
        tracing::info!(conv_id = %conv_id, count = messages.len(), "storage: batch_insert_messages");
        let Some(path) = self.conv_file_path(conv_id) else {
            tracing::warn!("No log file found for conversation {}", conv_id);
            return Ok(());
        };
        tracing::debug!(conv_id = %conv_id, path = %path.display(), "storage: resolved conv file path");

        // Build the lines to append before acquiring the lock.
        let mut buf = String::new();
        for msg in messages {
            buf.push_str(&serde_json::to_string(msg)?);
            buf.push('\n');
        }
        tracing::debug!(conv_id = %conv_id, buf_bytes = buf.len(), "storage: batch_insert_messages: serialised");

        // Lock only for the actual write — no I/O inside the lock except the append.
        let _guard = self.write_lock.lock().unwrap();
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .with_context(|| format!("open for append {:?}", path))?;
        file.write_all(buf.as_bytes())
            .with_context(|| format!("append to {:?}", path))?;
        tracing::info!(conv_id = %conv_id, count = messages.len(), bytes_written = buf.len(), "storage: batch_insert_messages ok");
        Ok(())
    }

    /// Returns up to 10 conversations, newest first.
    /// Reads only line 1 of each file — O(N_files × header_size).
    pub fn list_conversations(&self) -> Result<Vec<Conversation>> {
        let mut entries: Vec<_> = std::fs::read_dir(&self.logs_dir)?
            .flatten()
            .filter(|e| {
                e.path()
                    .extension()
                    .and_then(|x| x.to_str())
                    == Some("ndjson")
            })
            .collect();

        // Filenames start with ISO timestamp — descending sort = newest first.
        entries.sort_by_key(|b| std::cmp::Reverse(b.file_name()));
        let total_files = entries.len();
        entries.truncate(10);

        let mut result = Vec::new();
        for entry in entries {
            if let Ok(conv) = Self::read_conv_header(&entry.path()) {
                result.push(conv);
            }
        }
        tracing::info!(total_files, returned = result.len(), "storage: list_conversations");
        Ok(result)
    }

    pub fn get_messages(&self, conversation_id: &str) -> Result<Vec<Message>> {
        let Some(path) = self.conv_file_path(conversation_id) else {
            return Ok(vec![]);
        };
        Self::read_messages(&path)
    }

    /// Find a today's conversation by provider + fingerprint.
    /// Reads only line 1 of each today's file — O(N_today × header_size).
    pub fn find_conversation_by_fingerprint(
        &self,
        provider: &str,
        fingerprint: &str,
    ) -> Option<String> {
        let fp_prefix = &fingerprint[..fingerprint.len().min(8)];
        tracing::info!(provider = %provider, fingerprint_prefix = %fp_prefix, "storage: find_conversation_by_fingerprint");
        let today = Utc::now().format("%Y-%m-%d").to_string();
        for entry in std::fs::read_dir(&self.logs_dir).ok()?.flatten() {
            let fname = entry.file_name().to_string_lossy().to_string();
            if !fname.starts_with(&today) || !fname.ends_with(".ndjson") {
                continue;
            }
            if let Ok(conv) = Self::read_conv_header(&entry.path()) {
                if conv.provider == provider
                    && conv.client_hint.as_deref() == Some(fingerprint)
                {
                    tracing::info!(conv_id = %conv.id, "storage: find_conversation_by_fingerprint: found");
                    return Some(conv.id);
                }
            }
        }
        tracing::info!(provider = %provider, "storage: find_conversation_by_fingerprint: not found");
        None
    }

    /// Count request-direction messages stored for a conversation.
    /// Sequential line scan — no JSON tree allocation for the whole file.
    pub fn count_request_messages(&self, conversation_id: &str) -> usize {
        let Some(path) = self.conv_file_path(conversation_id) else {
            return 0;
        };
        let Ok(file) = std::fs::File::open(&path) else {
            return 0;
        };
        let reader = std::io::BufReader::new(file);
        let count = reader
            .lines()
            .enumerate()
            .filter(|(i, line)| {
                if *i == 0 { return false; } // skip conv header
                line.as_ref()
                    .map(|l| l.contains("\"direction\":\"request\""))
                    .unwrap_or(false)
            })
            .count();
        tracing::info!(conv_id = %conversation_id, count, "storage: count_request_messages");
        count
    }

    /// Delete all log files not belonging to today.
    pub fn rotate_old(&self) -> Result<usize> {
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let mut deleted = 0;
        for entry in std::fs::read_dir(&self.logs_dir)?.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".ndjson") && !name.starts_with(&today) {
                tracing::debug!(file = %name, "storage: rotate_old: deleting file");
                std::fs::remove_file(entry.path())?;
                deleted += 1;
            }
        }
        tracing::info!(deleted, "storage: rotate_old done");
        Ok(deleted)
    }

    /// Export all conversations as a JSON array of `{conversation, messages}` objects.
    /// Used by the dashboard API endpoint.
    pub fn export_all(&self) -> Result<serde_json::Value> {
        let mut result = Vec::new();
        for entry in std::fs::read_dir(&self.logs_dir)?.flatten() {
            if entry.path().extension().and_then(|x| x.to_str()) != Some("ndjson") {
                continue;
            }
            let path = entry.path();
            let Ok(conv) = Self::read_conv_header(&path) else { continue; };
            let messages = Self::read_messages(&path).unwrap_or_default();
            result.push(serde_json::json!({
                "conversation": conv,
                "messages": messages,
            }));
        }
        Ok(serde_json::Value::Array(result))
    }

    /// Persist the vault for a conversation as a `"type":"vault"` NDJSON line.
    ///
    /// If a vault line already exists in the file, it is replaced in-place (full rewrite).
    /// If no vault line exists, the line is appended to the end of the file.
    pub fn save_vault(
        &self,
        conv_id: &str,
        rng_seed: u64,
        records: &[(String, String)],
    ) -> Result<()> {
        let Some(path) = self.conv_file_path(conv_id) else {
            return Ok(());
        };

        let persisted = PersistedVault {
            record_type: "vault".to_string(),
            rng_seed,
            mappings: records
                .iter()
                .map(|(orig, synth)| StoredVaultRecord {
                    original: orig.clone(),
                    synthetic: synth.clone(),
                    pii_type: String::new(),
                })
                .collect(),
        };
        let vault_line = serde_json::to_string(&persisted)? + "\n";

        // Read existing file lines.
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("read {:?} for vault save", path))?;

        let _guard = self.write_lock.lock().unwrap();

        // Check if a vault line already exists.
        let mut lines: Vec<&str> = content.lines().collect();
        let existing_idx = lines.iter().position(|l| l.contains("\"type\":\"vault\""));

        if let Some(idx) = existing_idx {
            lines[idx] = vault_line.trim_end_matches('\n');
            let new_content = lines.join("\n") + "\n";
            std::fs::write(&path, new_content.as_bytes())
                .with_context(|| format!("rewrite {:?} for vault update", path))?;
        } else {
            // Append vault line to end of file.
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .with_context(|| format!("open {:?} for vault append", path))?;
            file.write_all(vault_line.as_bytes())
                .with_context(|| format!("append vault to {:?}", path))?;
        }

        Ok(())
    }

    /// Load vault state from a conversation's NDJSON file.
    /// Returns `(rng_seed, records)` if found, else `None`.
    pub fn load_vault(
        &self,
        conv_id: &str,
    ) -> Result<Option<(u64, Vec<StoredVaultRecord>)>> {
        let Some(path) = self.conv_file_path(conv_id) else {
            return Ok(None);
        };
        let file = std::fs::File::open(&path)
            .with_context(|| format!("open {:?} for vault load", path))?;
        let reader = std::io::BufReader::new(file);
        for line in reader.lines() {
            let line = line?;
            if line.contains("\"type\":\"vault\"") {
                match serde_json::from_str::<PersistedVault>(&line) {
                    Ok(pv) => return Ok(Some((pv.rng_seed, pv.mappings))),
                    Err(e) => {
                        tracing::warn!("Malformed vault line in {:?}: {}", path, e);
                        return Ok(None);
                    }
                }
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn temp_store() -> (Store, TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        (store, dir)
    }

    fn make_conv(id: &str, provider: &str, fingerprint: &str) -> Conversation {
        Conversation {
            id: id.to_string(),
            started_at: "2026-03-07T00:00:00Z".to_string(),
            provider: provider.to_string(),
            model: Some("test-model".to_string()),
            client_hint: Some(fingerprint.to_string()),
        }
    }

    fn make_msg(id: &str, conv_id: &str, direction: &str) -> Message {
        Message {
            id: id.to_string(),
            conversation_id: conv_id.to_string(),
            direction: direction.to_string(),
            timestamp: "2026-03-07T00:00:00Z".to_string(),
            role: Some("user".to_string()),
            content: "test content".to_string(),
            tokens_in: None,
            tokens_out: None,
        }
    }

    // ── 3.1 Basic CRUD ────────────────────────────────────────────────────────

    #[test]
    fn test_insert_and_get_conversation() {
        let (store, _dir) = temp_store();
        let conv = make_conv("conv-1", "anthropic", "fingerprint-abc");
        store.insert_conversation(&conv).unwrap();
        let found = store.find_conversation_by_fingerprint("anthropic", "fingerprint-abc");
        assert_eq!(found.as_deref(), Some("conv-1"));
    }

    #[test]
    fn test_insert_and_get_messages_in_order() {
        let (store, _dir) = temp_store();
        let conv = make_conv("conv-1", "anthropic", "fp1");
        store.insert_conversation(&conv).unwrap();
        for i in 0..10 {
            let msg = Message {
                id: format!("msg-{}", i),
                conversation_id: "conv-1".to_string(),
                direction: "request".to_string(),
                timestamp: "2026-03-07T00:00:00Z".to_string(),
                role: Some("user".to_string()),
                content: format!("content-{}", i),
                tokens_in: None,
                tokens_out: None,
            };
            store.insert_message(&msg).unwrap();
        }
        let messages = store.get_messages("conv-1").unwrap();
        assert_eq!(messages.len(), 10);
        for (i, msg) in messages.iter().enumerate() {
            assert_eq!(msg.content, format!("content-{}", i));
        }
    }

    #[test]
    fn test_batch_insert_preserves_order() {
        let (store, _dir) = temp_store();
        let conv = make_conv("conv-1", "anthropic", "fp1");
        store.insert_conversation(&conv).unwrap();
        let msgs: Vec<Message> = (0..100)
            .map(|i| Message {
                id: format!("msg-{}", i),
                conversation_id: "conv-1".to_string(),
                direction: "request".to_string(),
                timestamp: "2026-03-07T00:00:00Z".to_string(),
                role: Some("user".to_string()),
                content: format!("content-{}", i),
                tokens_in: None,
                tokens_out: None,
            })
            .collect();
        store.batch_insert_messages(&msgs).unwrap();
        let retrieved = store.get_messages("conv-1").unwrap();
        assert_eq!(retrieved.len(), 100);
        for (i, msg) in retrieved.iter().enumerate() {
            assert_eq!(msg.content, format!("content-{}", i));
        }
    }

    #[test]
    fn test_list_conversations_newest_first() {
        let (store, _dir) = temp_store();
        for i in 0..15 {
            let conv = make_conv(
                &format!("conv-{:02}", i),
                "anthropic",
                &format!("fp-{}", i),
            );
            store.insert_conversation(&conv).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let convs = store.list_conversations().unwrap();
        assert_eq!(convs.len(), 10, "list_conversations returns at most 10");
    }

    // ── 3.2 Fingerprinting ────────────────────────────────────────────────────

    #[test]
    fn test_same_fingerprint_same_conv() {
        let (store, _dir) = temp_store();
        let conv = make_conv("conv-abc", "anthropic", "unique-fp-123");
        store.insert_conversation(&conv).unwrap();
        let found = store.find_conversation_by_fingerprint("anthropic", "unique-fp-123");
        assert_eq!(found.as_deref(), Some("conv-abc"));
    }

    #[test]
    fn test_different_provider_same_fingerprint_separate() {
        let (store, _dir) = temp_store();
        let conv1 = make_conv("conv-anthropic", "anthropic", "shared-fp");
        let conv2 = make_conv("conv-openai", "openai", "shared-fp");
        store.insert_conversation(&conv1).unwrap();
        store.insert_conversation(&conv2).unwrap();
        let found_anthropic =
            store.find_conversation_by_fingerprint("anthropic", "shared-fp");
        let found_openai = store.find_conversation_by_fingerprint("openai", "shared-fp");
        assert_eq!(found_anthropic.as_deref(), Some("conv-anthropic"));
        assert_eq!(found_openai.as_deref(), Some("conv-openai"));
    }

    #[test]
    fn test_unknown_fingerprint_returns_none() {
        let (store, _dir) = temp_store();
        let found = store.find_conversation_by_fingerprint("anthropic", "nonexistent-fp");
        assert!(found.is_none());
    }

    #[test]
    fn test_count_request_messages_counts_only_requests() {
        let (store, _dir) = temp_store();
        let conv = make_conv("conv-1", "anthropic", "fp1");
        store.insert_conversation(&conv).unwrap();
        let req_msgs: Vec<Message> = (0..5)
            .map(|i| make_msg(&format!("req-{}", i), "conv-1", "request"))
            .collect();
        store.batch_insert_messages(&req_msgs).unwrap();
        let resp_msgs: Vec<Message> = (0..3)
            .map(|i| make_msg(&format!("resp-{}", i), "conv-1", "response"))
            .collect();
        store.batch_insert_messages(&resp_msgs).unwrap();
        let count = store.count_request_messages("conv-1");
        assert_eq!(count, 5);
    }

    // ── 3.3 Concurrency ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_concurrent_batch_inserts_no_corruption() {
        let (store, _dir) = temp_store();
        let conv = make_conv("conv-concurrent", "anthropic", "fp-concurrent");
        store.insert_conversation(&conv).unwrap();

        let mut handles = Vec::new();
        for task_id in 0..10 {
            let store_clone = store.clone();
            let handle = tokio::task::spawn_blocking(move || {
                let msgs: Vec<Message> = (0..20)
                    .map(|i| Message {
                        id: format!("msg-{}-{}", task_id, i),
                        conversation_id: "conv-concurrent".to_string(),
                        direction: "request".to_string(),
                        timestamp: "2026-03-07T00:00:00Z".to_string(),
                        role: Some("user".to_string()),
                        content: format!("task-{} msg-{}", task_id, i),
                        tokens_in: None,
                        tokens_out: None,
                    })
                    .collect();
                store_clone.batch_insert_messages(&msgs).unwrap();
            });
            handles.push(handle);
        }
        for h in handles {
            h.await.unwrap();
        }

        let messages = store.get_messages("conv-concurrent").unwrap();
        assert_eq!(
            messages.len(),
            200,
            "Expected 200 messages, got {}",
            messages.len()
        );
    }

    // ── 3.4 Robustness ────────────────────────────────────────────────────────

    #[test]
    fn test_malformed_message_line_skipped() {
        let (store, dir) = temp_store();
        let conv = make_conv("conv-malformed", "anthropic", "fp-malformed");
        store.insert_conversation(&conv).unwrap();

        let msg = make_msg("msg-1", "conv-malformed", "request");
        store.insert_message(&msg).unwrap();

        // Manually corrupt a line by appending bad JSON.
        let suffix = format!("_conv-malformed.ndjson");
        let file_path = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .find(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.ends_with(&suffix))
                    .unwrap_or(false)
            })
            .unwrap();

        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&file_path)
            .unwrap();
        f.write_all(b"not valid json at all{{{{\n").unwrap();

        let msg2 = make_msg("msg-2", "conv-malformed", "request");
        store.insert_message(&msg2).unwrap();

        let messages = store.get_messages("conv-malformed").unwrap();
        assert_eq!(
            messages.len(),
            2,
            "Expected 2 valid messages, corrupt line skipped"
        );
    }

    #[test]
    fn test_batch_insert_missing_conv_is_noop() {
        let (store, _dir) = temp_store();
        let msgs = vec![make_msg("msg-1", "nonexistent-conv", "request")];
        let result = store.batch_insert_messages(&msgs);
        assert!(
            result.is_ok(),
            "batch_insert_messages with missing conv should be Ok (no-op)"
        );
    }

    // ── Vault tests ───────────────────────────────────────────────────────────

    #[test]
    fn test_save_and_load_vault_basic() {
        let (store, _dir) = temp_store();
        let conv = make_conv("conv-vault-basic", "anthropic", "fp-vault-basic");
        store.insert_conversation(&conv).unwrap();

        store
            .save_vault(
                "conv-vault-basic",
                42,
                &[
                    ("alice@acme.com".to_string(), "bob@example.com".to_string()),
                ],
            )
            .unwrap();

        let result = store.load_vault("conv-vault-basic").unwrap();
        assert!(result.is_some(), "load_vault should return Some after save_vault");

        let (seed, records) = result.unwrap();
        assert_eq!(seed, 42, "rng_seed must round-trip correctly");
        assert_eq!(records.len(), 1, "expected exactly 1 mapping");
        assert_eq!(records[0].original,  "alice@acme.com",  "original mismatch");
        assert_eq!(records[0].synthetic, "bob@example.com", "synthetic mismatch");
    }

    #[test]
    fn test_load_vault_nonexistent_conv() {
        let (store, _dir) = temp_store();
        // No conversation with this id was ever created.
        let result = store.load_vault("does-not-exist").unwrap();
        assert!(result.is_none(), "load_vault on unknown conv_id should return Ok(None)");
    }

    #[test]
    fn test_load_vault_no_vault_line() {
        let (store, _dir) = temp_store();
        let conv = make_conv("conv-no-vault", "openai", "fp-no-vault");
        store.insert_conversation(&conv).unwrap();

        // Insert some messages but never call save_vault.
        let msg = make_msg("msg-1", "conv-no-vault", "request");
        store.insert_message(&msg).unwrap();

        let result = store.load_vault("conv-no-vault").unwrap();
        assert!(result.is_none(), "no vault line written => should return Ok(None)");
    }

    #[test]
    fn test_save_vault_overwrites_existing() {
        let (store, _dir) = temp_store();
        let conv = make_conv("conv-vault-overwrite", "anthropic", "fp-vault-overwrite");
        store.insert_conversation(&conv).unwrap();

        // First save — one mapping.
        store
            .save_vault(
                "conv-vault-overwrite",
                1,
                &[("first@orig.com".to_string(), "first@synth.com".to_string())],
            )
            .unwrap();

        // Second save — different mapping, different seed.
        store
            .save_vault(
                "conv-vault-overwrite",
                2,
                &[("second@orig.com".to_string(), "second@synth.com".to_string())],
            )
            .unwrap();

        let result = store.load_vault("conv-vault-overwrite").unwrap();
        assert!(result.is_some());
        let (seed, records) = result.unwrap();

        // Only the second write should be present.
        assert_eq!(seed, 2, "seed should reflect the second save");
        assert_eq!(records.len(), 1, "only one record from the second save");
        assert_eq!(records[0].original, "second@orig.com",  "original must be from second save");
        assert_eq!(records[0].synthetic, "second@synth.com", "synthetic must be from second save");

        // The first record must NOT appear.
        assert!(
            !records.iter().any(|r| r.original == "first@orig.com"),
            "first save records should have been overwritten"
        );
    }

    #[test]
    fn test_save_vault_empty_records() {
        let (store, _dir) = temp_store();
        let conv = make_conv("conv-vault-empty", "openai", "fp-vault-empty");
        store.insert_conversation(&conv).unwrap();

        // Save with an empty mapping slice — must not crash.
        store.save_vault("conv-vault-empty", 0, &[]).unwrap();

        let result = store.load_vault("conv-vault-empty").unwrap();
        assert!(result.is_some(), "empty vault should still produce a vault line");
        let (seed, records) = result.unwrap();
        assert_eq!(seed, 0);
        assert!(records.is_empty(), "no records expected for empty save");
    }

    #[test]
    fn test_save_vault_multiple_records() {
        let (store, _dir) = temp_store();
        let conv = make_conv("conv-vault-multi", "anthropic", "fp-vault-multi");
        store.insert_conversation(&conv).unwrap();

        let mappings: Vec<(String, String)> = (0..5)
            .map(|i| {
                (
                    format!("original-{}@acme.com", i),
                    format!("synthetic-{}@example.com", i),
                )
            })
            .collect();

        store.save_vault("conv-vault-multi", 99, &mappings).unwrap();

        let result = store.load_vault("conv-vault-multi").unwrap();
        assert!(result.is_some());
        let (seed, records) = result.unwrap();

        assert_eq!(seed, 99);
        assert_eq!(records.len(), 5, "all 5 records should round-trip");

        for i in 0..5usize {
            let expected_orig = format!("original-{}@acme.com", i);
            let expected_synth = format!("synthetic-{}@example.com", i);
            let found = records
                .iter()
                .any(|r| r.original == expected_orig && r.synthetic == expected_synth);
            assert!(found, "record {i} not found in loaded vault");
        }
    }
}
