use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use crate::storage::Store;

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

    /// Get existing vault or load from storage on cache miss, falling back to a new empty vault.
    pub fn get_or_create_with_store(&self, conv_id: &str, store: &Store) -> VaultHandle {
        // Check in-memory cache first.
        {
            let mut map = self.vaults.lock().unwrap();
            if let Some(entry) = map.get_mut(conv_id) {
                entry.last_accessed = Instant::now();
                return Arc::clone(&entry.handle);
            }
        }

        // Cache miss — try to load from storage.
        let vault = match store.load_vault(conv_id) {
            Ok(Some((seed, records))) => {
                tracing::info!(conv_id = %conv_id, mappings = records.len(), "vault: restored from storage");
                let vault_records: Vec<VaultRecord> = records
                    .into_iter()
                    .map(|r| VaultRecord {
                        original: r.original,
                        synthetic: r.synthetic,
                        pii_type: PiiType::Custom(r.pii_type),
                    })
                    .collect();
                PiiVault::from_records(conv_id, seed, vault_records)
            }
            Ok(None) => {
                tracing::debug!(conv_id = %conv_id, "vault: no persisted vault found, creating fresh");
                PiiVault::new(conv_id)
            }
            Err(e) => {
                tracing::warn!(conv_id = %conv_id, err = %e, "vault: load_vault failed, creating fresh");
                PiiVault::new(conv_id)
            }
        };

        let handle = Arc::new(RwLock::new(vault));
        let mut map = self.vaults.lock().unwrap();
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

    // ── New tests ──────────────────────────────────────────────────────────────

    #[test]
    fn test_replace_originals_basic() {
        let mut vault = PiiVault::new("test-conv-ro-basic");
        vault.add_mapping(
            "alice@acme.com".to_string(),
            "bob@example.com".to_string(),
            &PiiType::Email,
        );
        let result = vault.replace_originals("send to alice@acme.com please");
        assert!(result.contains("bob@example.com"), "synthetic not present: {result}");
        assert!(!result.contains("alice@acme.com"), "original still present: {result}");
    }

    #[test]
    fn test_replace_originals_empty_vault() {
        let vault = PiiVault::new("test-conv-ro-empty");
        let input = "nothing to replace here";
        let result = vault.replace_originals(input);
        assert_eq!(result, input);
    }

    #[test]
    fn test_replace_originals_multiple_mappings() {
        let mut vault = PiiVault::new("test-conv-ro-multi");
        vault.add_mapping(
            "alice@acme.com".to_string(),
            "syn-email@example.com".to_string(),
            &PiiType::Email,
        );
        vault.add_mapping(
            "123-45-6789".to_string(),
            "999-00-0001".to_string(),
            &PiiType::Ssn,
        );
        vault.add_mapping(
            "Alice Smith".to_string(),
            "Synthetic Name".to_string(),
            &PiiType::PersonName,
        );

        let text = "Email alice@acme.com, SSN 123-45-6789, Name Alice Smith.";
        let result = vault.replace_originals(text);

        assert!(result.contains("syn-email@example.com"), "email not replaced: {result}");
        assert!(result.contains("999-00-0001"), "ssn not replaced: {result}");
        assert!(result.contains("Synthetic Name"), "name not replaced: {result}");

        assert!(!result.contains("alice@acme.com"), "original email still present: {result}");
        assert!(!result.contains("123-45-6789"), "original ssn still present: {result}");
        assert!(!result.contains("Alice Smith"), "original name still present: {result}");
    }

    #[test]
    fn test_full_round_trip_forward_and_back() {
        let mut vault = PiiVault::new("test-conv-roundtrip");
        vault.add_mapping(
            "alice@acme.com".to_string(),
            "bob@example.com".to_string(),
            &PiiType::Email,
        );

        let original_text = "Please contact alice@acme.com for support.";
        let with_synthetic = vault.replace_originals(original_text);
        assert!(with_synthetic.contains("bob@example.com"));

        let (restored, any) = vault.replace_synthetics(&with_synthetic);
        assert!(any);
        assert_eq!(restored, original_text);
    }

    #[test]
    fn test_replace_synthetics_no_match() {
        let mut vault = PiiVault::new("test-conv-rs-nomatch");
        vault.add_mapping(
            "alice@acme.com".to_string(),
            "bob@example.com".to_string(),
            &PiiType::Email,
        );

        let text = "no synthetic tokens here at all";
        let (result, any_match) = vault.replace_synthetics(text);
        assert!(!any_match);
        assert_eq!(result, text);
    }

    #[test]
    fn test_replace_synthetics_multiple_matches() {
        let mut vault = PiiVault::new("test-conv-rs-multi");
        vault.add_mapping(
            "alice@acme.com".to_string(),
            "syn-email@example.com".to_string(),
            &PiiType::Email,
        );
        vault.add_mapping(
            "Bob Jones".to_string(),
            "Fake Person".to_string(),
            &PiiType::PersonName,
        );

        let text = "Reply to syn-email@example.com, re: Fake Person.";
        let (result, any_match) = vault.replace_synthetics(text);

        assert!(any_match);
        assert!(result.contains("alice@acme.com"), "email not restored: {result}");
        assert!(result.contains("Bob Jones"), "name not restored: {result}");
        assert!(!result.contains("syn-email@example.com"), "synthetic email still present: {result}");
        assert!(!result.contains("Fake Person"), "synthetic name still present: {result}");
    }

    #[test]
    fn test_vault_from_records() {
        let records = vec![VaultRecord {
            original: "alice@acme.com".to_string(),
            synthetic: "bob@example.com".to_string(),
            pii_type: PiiType::Email,
        }];
        let vault = PiiVault::from_records("conv-test", 12345u64, records);

        assert_eq!(vault.get_synthetic("alice@acme.com"), Some("bob@example.com"));
        assert!(!vault.is_empty());
    }

    #[test]
    fn test_vault_to_records() {
        let mut vault = PiiVault::new("test-conv-to-records");
        vault.add_mapping(
            "alice@acme.com".to_string(),
            "bob@example.com".to_string(),
            &PiiType::Email,
        );

        let records = vault.to_records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].synthetic, "bob@example.com");
        assert_eq!(records[0].original, "alice@acme.com");
    }

    #[test]
    fn test_mapping_count() {
        let mut vault = PiiVault::new("test-conv-count");
        vault.add_mapping("a@a.com".to_string(), "x@x.com".to_string(), &PiiType::Email);
        vault.add_mapping("b@b.com".to_string(), "y@y.com".to_string(), &PiiType::Email);
        vault.add_mapping("c@c.com".to_string(), "z@z.com".to_string(), &PiiType::Email);
        assert_eq!(vault.mapping_count(), 3);
    }

    #[test]
    fn test_pairs_iter() {
        let mut vault = PiiVault::new("test-conv-pairs");
        vault.add_mapping(
            "orig-one".to_string(),
            "syn-one".to_string(),
            &PiiType::PersonName,
        );
        vault.add_mapping(
            "orig-two".to_string(),
            "syn-two".to_string(),
            &PiiType::PersonName,
        );

        let collected: Vec<(&str, &str)> = vault.pairs().collect();
        assert_eq!(collected.len(), 2);

        let has_one = collected.iter().any(|&(o, s)| o == "orig-one" && s == "syn-one");
        let has_two = collected.iter().any(|&(o, s)| o == "orig-two" && s == "syn-two");
        assert!(has_one, "pair (orig-one, syn-one) not found");
        assert!(has_two, "pair (orig-two, syn-two) not found");
    }

    #[test]
    fn test_synthetic_key_first_chars() {
        let mut vault = PiiVault::new("test-conv-firstchars");
        vault.add_mapping("orig-a".to_string(), "Alpha_token".to_string(), &PiiType::PersonName);
        vault.add_mapping("orig-b".to_string(), "Beta_token".to_string(), &PiiType::PersonName);
        vault.add_mapping("orig-c".to_string(), "Gamma_token".to_string(), &PiiType::PersonName);

        let chars: std::collections::HashSet<char> = vault.synthetic_key_first_chars().collect();
        assert!(chars.contains(&'A'), "missing 'A': {chars:?}");
        assert!(chars.contains(&'B'), "missing 'B': {chars:?}");
        assert!(chars.contains(&'G'), "missing 'G': {chars:?}");
    }

    #[test]
    fn test_registry_creates_distinct_vaults() {
        let registry = VaultRegistry::new(Duration::from_secs(60));
        let handle1 = registry.get_or_create("conv-1");
        let handle2 = registry.get_or_create("conv-2");

        let id1 = handle1.read().unwrap().conversation_id.clone();
        let id2 = handle2.read().unwrap().conversation_id.clone();

        assert_eq!(id1, "conv-1");
        assert_eq!(id2, "conv-2");
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_registry_get_or_create_idempotent() {
        let registry = VaultRegistry::new(Duration::from_secs(60));

        let handle_first = registry.get_or_create("same-conv");
        handle_first.write().unwrap().add_mapping(
            "original@test.com".to_string(),
            "synthetic@test.com".to_string(),
            &PiiType::Email,
        );

        let handle_second = registry.get_or_create("same-conv");
        let synthetic = handle_second
            .read()
            .unwrap()
            .get_synthetic("original@test.com")
            .map(|s| s.to_string());

        assert_eq!(synthetic, Some("synthetic@test.com".to_string()),
            "second handle does not see the mapping added via first handle");
    }

    #[test]
    fn test_replace_synthetics_partial_overlap() {
        let mut vault = PiiVault::new("test-conv-overlap");
        vault.add_mapping(
            "short-original".to_string(),
            "AAA_TOKEN".to_string(),
            &PiiType::Custom("test".to_string()),
        );
        vault.add_mapping(
            "long-original".to_string(),
            "AAA_TOKEN_LONG".to_string(),
            &PiiType::Custom("test".to_string()),
        );

        let text = "value is AAA_TOKEN_LONG here";
        let (result, any_match) = vault.replace_synthetics(text);

        assert!(any_match);
        // LeftmostLongest: "AAA_TOKEN_LONG" must win over "AAA_TOKEN"
        assert!(result.contains("long-original"), "longer token not matched: {result}");
        assert!(!result.contains("short-original"), "shorter token matched instead: {result}");
        assert!(!result.contains("AAA_TOKEN"), "synthetic still present: {result}");
    }
}
