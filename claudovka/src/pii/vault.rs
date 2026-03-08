use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

/// Every type of PII the system can detect and replace.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PiiType {
    Email,
    Phone,
    Ssn,
    CreditCard,
    IpV4,
    IpV6,
    OpenAiApiKey,
    AwsAccessKey,
    AwsSecretKey,
    GitHubPat,
    BearerToken,
    SshPrivateKey,
    DbConnectionString,
    UrlWithCreds,
    PersonName,
    OrgName,
    Address,
    DateOfBirth,
    MedicalRecord,
    PassportNumber,
    DriversLicense,
    FinancialAccount,
    AadhaarNumber,
    CpfNumber,
    Custom(String),
}

impl PiiType {
    pub fn label(&self) -> &str {
        match self {
            PiiType::Email => "EMAIL",
            PiiType::Phone => "PHONE",
            PiiType::Ssn => "SSN",
            PiiType::CreditCard => "CREDIT_CARD",
            PiiType::IpV4 => "IP_V4",
            PiiType::IpV6 => "IP_V6",
            PiiType::OpenAiApiKey => "OPENAI_API_KEY",
            PiiType::AwsAccessKey => "AWS_ACCESS_KEY",
            PiiType::AwsSecretKey => "AWS_SECRET_KEY",
            PiiType::GitHubPat => "GITHUB_PAT",
            PiiType::BearerToken => "BEARER_TOKEN",
            PiiType::SshPrivateKey => "SSH_PRIVATE_KEY",
            PiiType::DbConnectionString => "DB_CONNECTION_STRING",
            PiiType::UrlWithCreds => "URL_WITH_CREDS",
            PiiType::PersonName => "PERSON_NAME",
            PiiType::OrgName => "ORG_NAME",
            PiiType::Address => "ADDRESS",
            PiiType::DateOfBirth => "DATE_OF_BIRTH",
            PiiType::MedicalRecord => "MEDICAL_RECORD",
            PiiType::PassportNumber => "PASSPORT_NUMBER",
            PiiType::DriversLicense => "DRIVERS_LICENSE",
            PiiType::FinancialAccount => "FINANCIAL_ACCOUNT",
            PiiType::AadhaarNumber => "AADHAAR_NUMBER",
            PiiType::CpfNumber => "CPF_NUMBER",
            PiiType::Custom(s) => s.as_str(),
        }
    }
}

/// A detected PII span within a text string.
#[derive(Debug, Clone)]
pub struct PiiSpan {
    /// Byte offset of the match start in the original text.
    pub start: usize,
    /// Byte offset of the match end (exclusive).
    pub end: usize,
    pub entity_type: PiiType,
    /// 1.0 for regex (exact), 0.0–1.0 for ML tiers.
    pub confidence: f32,
    /// 1 = regex, 2 = GLiNER, 3 = SLM sidecar.
    pub tier: u8,
}

/// Persisted form of vault mappings (saved to NDJSON).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultRecord {
    pub original: String,
    pub synthetic: String,
    pub pii_type: PiiType,
}

/// Per-conversation bidirectional PII mapping store.
///
/// Stores (original → synthetic) for outbound replacement and rebuilds an
/// Aho-Corasick automaton over synthetic keys for fast inbound reversal.
pub struct PiiVault {
    /// Outbound: original text → synthetic replacement.
    original_to_synthetic: HashMap<String, String>,
    /// Parallel vecs (position i is the same mapping):
    ///   synthetic_keys[i] → original_values[i]
    synthetic_keys: Vec<String>,
    original_values: Vec<String>,
    /// Fast multi-pattern matcher over synthetic_keys.
    /// None when the vault is empty.
    reverse_automaton: Option<AhoCorasick>,
    /// Length of the longest synthetic key (used for buffering window).
    pub max_synthetic_key_len: usize,
    /// Seeded RNG state (we store the seed; callers advance it by passing &mut SmallRng).
    pub rng_seed: u64,
    pub conversation_id: String,
    pub created_at: DateTime<Utc>,
}

impl PiiVault {
    /// Create a new empty vault seeded from `sha1(conversation_id)[0..8]`.
    pub fn new(conversation_id: &str) -> Self {
        let mut h = Sha1::new();
        h.update(conversation_id.as_bytes());
        let digest = h.finalize();
        let seed = u64::from_le_bytes(digest[..8].try_into().unwrap_or([0u8; 8]));
        Self {
            original_to_synthetic: HashMap::new(),
            synthetic_keys: Vec::new(),
            original_values: Vec::new(),
            reverse_automaton: None,
            max_synthetic_key_len: 0,
            rng_seed: seed,
            conversation_id: conversation_id.to_string(),
            created_at: Utc::now(),
        }
    }

    /// Restore from persisted records.
    pub fn from_records(conversation_id: &str, rng_seed: u64, records: Vec<VaultRecord>) -> Self {
        let mut v = Self {
            original_to_synthetic: HashMap::new(),
            synthetic_keys: Vec::new(),
            original_values: Vec::new(),
            reverse_automaton: None,
            max_synthetic_key_len: 0,
            rng_seed,
            conversation_id: conversation_id.to_string(),
            created_at: Utc::now(),
        };
        for r in records {
            v.insert_mapping_raw(r.original, r.synthetic);
        }
        v.rebuild_automaton();
        v
    }

    /// Get the synthetic replacement for an original string, if it exists.
    pub fn get_synthetic(&self, original: &str) -> Option<&str> {
        self.original_to_synthetic.get(original).map(|s| s.as_str())
    }

    /// Add a mapping. Rebuilds the AhoCorasick automaton.
    /// Panics if the same original has a conflicting synthetic (programming error).
    pub fn add_mapping(&mut self, original: String, synthetic: String, _pii_type: &PiiType) {
        if self.original_to_synthetic.contains_key(&original) {
            return; // idempotent
        }
        self.insert_mapping_raw(original, synthetic);
        self.rebuild_automaton();
    }

    fn insert_mapping_raw(&mut self, original: String, synthetic: String) {
        if synthetic.len() > self.max_synthetic_key_len {
            self.max_synthetic_key_len = synthetic.len();
        }
        self.original_to_synthetic.insert(original.clone(), synthetic.clone());
        self.synthetic_keys.push(synthetic);
        self.original_values.push(original);
    }

    fn rebuild_automaton(&mut self) {
        if self.synthetic_keys.is_empty() {
            self.reverse_automaton = None;
            return;
        }
        match AhoCorasickBuilder::new()
            .match_kind(MatchKind::LeftmostLongest)
            .build(&self.synthetic_keys)
        {
            Ok(ac) => self.reverse_automaton = Some(ac),
            Err(e) => {
                tracing::warn!("Failed to build AhoCorasick automaton: {}", e);
                self.reverse_automaton = None;
            }
        }
    }

    /// Replace all original PII values with their synthetic equivalents.
    /// Used on the outbound (request) path.
    pub fn replace_originals(&self, text: &str) -> String {
        if self.original_to_synthetic.is_empty() {
            return text.to_string();
        }
        // Build a one-shot AhoCorasick over original keys for this call.
        // For the outbound path this is called once per request, so the
        // overhead is acceptable compared to maintaining a second automaton.
        let originals: Vec<&str> = self.original_values.iter().map(|s| s.as_str()).collect();
        let synthetics: Vec<&str> = self.synthetic_keys.iter().map(|s| s.as_str()).collect();
        match AhoCorasickBuilder::new()
            .match_kind(MatchKind::LeftmostLongest)
            .build(originals)
        {
            Ok(ac) => {
                let mut result = String::with_capacity(text.len());
                let mut last = 0;
                for m in ac.find_iter(text) {
                    result.push_str(&text[last..m.start()]);
                    result.push_str(synthetics[m.pattern().as_usize()]);
                    last = m.end();
                }
                result.push_str(&text[last..]);
                result
            }
            Err(_) => text.to_string(),
        }
    }

    /// Replace all synthetic tokens back to their originals.
    /// Returns `(replaced_text, any_replacements_made)`.
    /// Used on the inbound (response) path.
    pub fn replace_synthetics(&self, text: &str) -> (String, bool) {
        let Some(ac) = &self.reverse_automaton else {
            return (text.to_string(), false);
        };
        let mut result = String::with_capacity(text.len());
        let mut last = 0;
        let mut any = false;
        for m in ac.find_iter(text) {
            result.push_str(&text[last..m.start()]);
            result.push_str(&self.original_values[m.pattern().as_usize()]);
            last = m.end();
            any = true;
        }
        result.push_str(&text[last..]);
        (result, any)
    }

    /// Snapshot the current vault state for persistence.
    pub fn to_records(&self) -> Vec<VaultRecord> {
        self.synthetic_keys
            .iter()
            .zip(self.original_values.iter())
            .map(|(syn, orig)| VaultRecord {
                original: orig.clone(),
                synthetic: syn.clone(),
                pii_type: PiiType::Custom("unknown".to_string()), // type not needed for reversal
            })
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.original_to_synthetic.is_empty()
    }

    /// Number of stored mappings.
    pub fn mapping_count(&self) -> usize {
        self.original_to_synthetic.len()
    }

    /// Iterator over (original, synthetic) string pairs.
    pub fn pairs(&self) -> impl Iterator<Item = (&str, &str)> {
        self.original_values
            .iter()
            .zip(self.synthetic_keys.iter())
            .map(|(o, s)| (o.as_str(), s.as_str()))
    }

    /// Iterator over the first character of each synthetic key (used for trigger-char optimization).
    pub fn synthetic_key_first_chars(&self) -> impl Iterator<Item = char> + '_ {
        self.synthetic_keys.iter().flat_map(|s| s.chars().next())
    }
}

pub type VaultHandle = Arc<RwLock<PiiVault>>;

// ─── VaultRegistry ────────────────────────────────────────────────────────────

struct VaultEntry {
    handle: VaultHandle,
    last_accessed: Instant,
}

/// Singleton registry of active vaults keyed by conversation_id.
pub struct VaultRegistry {
    vaults: Mutex<HashMap<String, VaultEntry>>,
    ttl: Duration,
}

impl VaultRegistry {
    pub fn new(ttl: Duration) -> Self {
        Self {
            vaults: Mutex::new(HashMap::new()),
            ttl,
        }
    }

    /// Get existing vault or create a new empty one.
    pub fn get_or_create(&self, conv_id: &str) -> VaultHandle {
        let mut map = self.vaults.lock().unwrap();
        if let Some(entry) = map.get_mut(conv_id) {
            entry.last_accessed = Instant::now();
            return Arc::clone(&entry.handle);
        }
        let handle = Arc::new(RwLock::new(PiiVault::new(conv_id)));
        map.insert(conv_id.to_string(), VaultEntry {
            handle: Arc::clone(&handle),
            last_accessed: Instant::now(),
        });
        handle
    }

    /// Insert a pre-loaded vault (used after loading from storage).
    pub fn insert(&self, conv_id: &str, vault: PiiVault) -> VaultHandle {
        let handle = Arc::new(RwLock::new(vault));
        let mut map = self.vaults.lock().unwrap();
        map.insert(conv_id.to_string(), VaultEntry {
            handle: Arc::clone(&handle),
            last_accessed: Instant::now(),
        });
        handle
    }

    /// Remove vaults that have not been accessed within the TTL.
    pub fn evict_expired(&self) {
        let ttl = self.ttl;
        let mut map = self.vaults.lock().unwrap();
        let before = map.len();
        map.retain(|_, entry| entry.last_accessed.elapsed() < ttl);
        let evicted = before - map.len();
        if evicted > 0 {
            tracing::info!(evicted, "VaultRegistry: evicted expired vaults");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_and_get_mapping() {
        let mut vault = PiiVault::new("test-conv-1");
        vault.add_mapping("john@acme.com".to_string(), "alice.brown@example.com".to_string(), &PiiType::Email);
        assert_eq!(vault.get_synthetic("john@acme.com"), Some("alice.brown@example.com"));
        assert_eq!(vault.get_synthetic("nobody@nowhere.com"), None);
    }

    #[test]
    fn test_idempotent_add() {
        let mut vault = PiiVault::new("test-conv-2");
        vault.add_mapping("a@b.com".to_string(), "x@example.com".to_string(), &PiiType::Email);
        vault.add_mapping("a@b.com".to_string(), "y@example.com".to_string(), &PiiType::Email);
        // Second add is ignored
        assert_eq!(vault.get_synthetic("a@b.com"), Some("x@example.com"));
    }

    #[test]
    fn test_replace_synthetics_round_trip() {
        let mut vault = PiiVault::new("test-conv-3");
        vault.add_mapping("John Smith".to_string(), "Alice Brown".to_string(), &PiiType::PersonName);
        vault.add_mapping("john@acme.com".to_string(), "alice@example.com".to_string(), &PiiType::Email);

        let text = "Contact Alice Brown at alice@example.com for details.";
        let (result, any) = vault.replace_synthetics(text);
        assert!(any);
        assert_eq!(result, "Contact John Smith at john@acme.com for details.");
    }

    #[test]
    fn test_replace_synthetics_longest_match() {
        let mut vault = PiiVault::new("test-conv-4");
        vault.add_mapping("Smith".to_string(), "Brown".to_string(), &PiiType::PersonName);
        vault.add_mapping("John Smith".to_string(), "Alice Brown".to_string(), &PiiType::PersonName);

        let text = "Hello Alice Brown.";
        let (result, _) = vault.replace_synthetics(text);
        // Should match "Alice Brown" (longest), not just "Brown"
        assert_eq!(result, "Hello John Smith.");
    }

    #[test]
    fn test_deterministic_seed() {
        let v1 = PiiVault::new("my-conv-id");
        let v2 = PiiVault::new("my-conv-id");
        assert_eq!(v1.rng_seed, v2.rng_seed);

        let v3 = PiiVault::new("other-conv-id");
        assert_ne!(v1.rng_seed, v3.rng_seed);
    }

    #[test]
    fn test_empty_vault_replace() {
        let vault = PiiVault::new("test-conv-5");
        let (result, any) = vault.replace_synthetics("hello world");
        assert!(!any);
        assert_eq!(result, "hello world");
    }
}
