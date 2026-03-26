use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind};
use serde::{Deserialize, Serialize};
use sha1::{Digest as Sha1Digest, Sha1};
use sha2::{Digest as Sha2Digest, Sha256};
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
    /// Detection tier (1=regex, 2=NER, 3=SLM). 0 when unknown (legacy data).
    #[serde(default)]
    pub tier: u8,
    /// Detection confidence. 0.0 when unknown (legacy data).
    #[serde(default)]
    pub confidence: f32,
    /// XML token ID (8-char base62). Empty string for legacy records.
    #[serde(default)]
    pub token_id: String,
    /// Bare display value (synthetic without XML wrapper). Empty for legacy records.
    #[serde(default)]
    pub display_value: String,
}

/// Generate a deterministic 8-character base62 token ID.
///
/// Computes SHA-256(conversation_id + ":" + entity_index), takes the first 6 bytes,
/// and encodes them as base62 (`0-9A-Za-z`), yielding exactly 8 characters.
pub fn generate_token_id(conversation_id: &str, entity_index: u64) -> String {
    let input = format!("{}:{}", conversation_id, entity_index);
    let mut hasher = Sha256::new();
    Sha2Digest::update(&mut hasher, input.as_bytes());
    let digest = hasher.finalize();
    // Take first 6 bytes → 48-bit value → base62 encode to 8 chars
    let val: u64 = u64::from_be_bytes([
        digest[0], digest[1], digest[2], digest[3], digest[4], digest[5], 0, 0,
    ]) >> 16;
    base62_encode_6bytes(val)
}

/// Assemble the canonical XML token string.
pub fn xml_token(token_id: &str, display_value: &str) -> String {
    format!("<pii id=\"{}\">{}</pii>", token_id, display_value)
}

const BASE62: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// Encode a 48-bit value (first 6 bytes) as exactly 8 base62 characters.
fn base62_encode_6bytes(mut val: u64) -> String {
    let mut chars = [0u8; 8];
    for i in (0..8).rev() {
        chars[i] = BASE62[(val % 62) as usize];
        val /= 62;
    }
    String::from_utf8(chars.to_vec()).expect("base62 chars are valid UTF-8")
}

/// Per-conversation bidirectional PII mapping store.
///
/// Stores (original → synthetic) for outbound replacement and rebuilds an
/// Aho-Corasick automaton over synthetic keys for fast inbound reversal.
pub struct PiiVault {
    /// Outbound: original text → synthetic replacement.
    original_to_synthetic: HashMap<String, String>,
    /// Parallel vecs (position i is the same mapping):
    ///   synthetic_keys[i] → original_values[i] → pii_type_labels[i] → tiers[i] → confidences[i] → token_ids[i]
    synthetic_keys: Vec<String>,
    original_values: Vec<String>,
    pii_type_labels: Vec<String>,
    tiers: Vec<u8>,
    confidences: Vec<f32>,
    /// TOKEN_ID for each mapping (empty string for legacy entries without token_id).
    token_ids: Vec<String>,
    /// Fast multi-pattern matcher over synthetic_keys.
    /// None when the vault is empty.
    reverse_automaton: Option<AhoCorasick>,
    /// Length of the longest synthetic key (used for buffering window).
    pub(crate) max_synthetic_key_len: usize,
    /// Seeded RNG state (we store the seed; callers advance it by passing &mut SmallRng).
    pub(crate) rng_seed: u64,
    #[cfg_attr(not(test), allow(dead_code))]
    conversation_id: String,
    /// XML token → original. Key: `<pii id="TOKEN_ID">DISPLAY_VALUE</pii>`. Level 1 cascade.
    pub full_token_to_original: HashMap<String, String>,
    /// TOKEN_ID (8-char base62) → original. Level 2 cascade.
    pub token_id_to_original: HashMap<String, String>,
    /// Bare display value → original. Level 3 cascade.
    pub display_value_to_original: HashMap<String, String>,
}

impl PiiVault {
    /// Create a new empty vault seeded from `sha1(conversation_id)[0..8]`.
    pub fn new(conversation_id: &str) -> Self {
        let mut h = Sha1::new();
        Sha1Digest::update(&mut h, conversation_id.as_bytes());
        let digest = h.finalize();
        let seed = u64::from_le_bytes(digest[..8].try_into().unwrap_or([0u8; 8]));
        Self {
            original_to_synthetic: HashMap::new(),
            synthetic_keys: Vec::new(),
            original_values: Vec::new(),
            pii_type_labels: Vec::new(),
            tiers: Vec::new(),
            confidences: Vec::new(),
            token_ids: Vec::new(),
            reverse_automaton: None,
            max_synthetic_key_len: 0,
            rng_seed: seed,
            conversation_id: conversation_id.to_string(),
            full_token_to_original: HashMap::new(),
            token_id_to_original: HashMap::new(),
            display_value_to_original: HashMap::new(),
        }
    }

    /// Restore from persisted records.
    pub fn from_records(conversation_id: &str, rng_seed: u64, records: Vec<VaultRecord>) -> Self {
        let mut v = Self {
            original_to_synthetic: HashMap::new(),
            synthetic_keys: Vec::new(),
            original_values: Vec::new(),
            pii_type_labels: Vec::new(),
            tiers: Vec::new(),
            confidences: Vec::new(),
            token_ids: Vec::new(),
            reverse_automaton: None,
            max_synthetic_key_len: 0,
            rng_seed,
            conversation_id: conversation_id.to_string(),
            full_token_to_original: HashMap::new(),
            token_id_to_original: HashMap::new(),
            display_value_to_original: HashMap::new(),
        };
        for r in records {
            let original = r.original.clone();
            let synthetic = r.synthetic.clone();
            let token_id = r.token_id.clone();
            let display_value = if r.display_value.is_empty() { synthetic.clone() } else { r.display_value.clone() };
            v.insert_mapping_raw_with_token_id(original.clone(), synthetic.clone(), r.pii_type.label().to_string(), r.tier, r.confidence, token_id.clone());
            // Populate index maps for records with token_id
            if !token_id.is_empty() {
                let full_token = xml_token(&token_id, &display_value);
                v.full_token_to_original.insert(full_token, original.clone());
                v.token_id_to_original.insert(token_id, original.clone());
                v.display_value_to_original.insert(display_value, original);
            }
        }
        v.rebuild_automaton();
        v
    }

    /// Get the synthetic replacement for an original string, if it exists.
    pub fn get_synthetic(&self, original: &str) -> Option<&str> {
        self.original_to_synthetic.get(original).map(|s| s.as_str())
    }

    /// Add a mapping. Rebuilds the AhoCorasick automaton.
    pub fn add_mapping(&mut self, original: String, synthetic: String, pii_type: &PiiType, tier: u8, confidence: f32) {
        tracing::trace!(
            original_len = original.len(),
            synthetic_len = synthetic.len(),
            pii_type = pii_type.label(),
            tier,
            "vault: add_mapping enter"
        );
        if self.original_to_synthetic.contains_key(&original) {
            return; // idempotent
        }
        self.insert_mapping_raw(original, synthetic, pii_type.label().to_string(), tier, confidence);
        self.rebuild_automaton();
        tracing::debug!(
            mapping_count = self.mapping_count(),
            max_key_len = self.max_synthetic_key_len,
            pii_type = pii_type.label(),
            "vault: mapping added"
        );
    }

    /// Add a mapping with an externally-computed token_id and display_value.
    ///
    /// Populates all three index HashMaps in addition to the core mapping.
    /// The core `original_to_synthetic` insert is idempotent (skipped on duplicate original),
    /// but the index HashMaps are always populated so cascade lookups work even when
    /// `get_or_create` was called before this method for the same original.
    pub fn add_mapping_with_token_id(
        &mut self,
        original: &str,
        display_value: &str,
        token_id: &str,
        pii_type: &PiiType,
        tier: u8,
        confidence: f32,
    ) {
        // Compute full_token unconditionally — it is needed in both branches below.
        let full_token = xml_token(token_id, display_value);
        if !self.original_to_synthetic.contains_key(original) {
            self.insert_mapping_raw_with_token_id(
                original.to_string(),
                display_value.to_string(),
                pii_type.label().to_string(),
                tier,
                confidence,
                token_id.to_string(),
            );
            self.rebuild_automaton();
            tracing::debug!(
                mapping_count = self.mapping_count(),
                token_id,
                pii_type = pii_type.label(),
                "vault: add_mapping_with_token_id: new mapping inserted"
            );
        } else {
            tracing::debug!(
                token_id,
                "vault: add_mapping_with_token_id: original already mapped, skipping core insert"
            );
        }
        // Always populate index maps — inserts are idempotent by key so re-insertion on
        // true duplicates is safe. This ensures cascade lookups work even when the
        // original was first seen via get_or_create (which does not populate index maps).
        self.full_token_to_original.insert(full_token, original.to_string());
        self.token_id_to_original.insert(token_id.to_string(), original.to_string());
        self.display_value_to_original.insert(display_value.to_string(), original.to_string());
    }

    /// Return the conversation ID this vault was created for.
    pub fn conversation_id(&self) -> &str {
        &self.conversation_id
    }

    /// Cascade Level 2 lookup: find original PII by TOKEN_ID.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn get_by_token_id(&self, token_id: &str) -> Option<&str> {
        self.token_id_to_original.get(token_id).map(|s| s.as_str())
    }

    /// Cascade Level 3 lookup: find original PII by bare display value.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn get_by_display_value(&self, display_value: &str) -> Option<&str> {
        self.display_value_to_original.get(display_value).map(|s| s.as_str())
    }

    fn insert_mapping_raw(&mut self, original: String, synthetic: String, pii_type_label: String, tier: u8, confidence: f32) {
        self.insert_mapping_raw_with_token_id(original, synthetic, pii_type_label, tier, confidence, String::new());
    }

    fn insert_mapping_raw_with_token_id(&mut self, original: String, synthetic: String, pii_type_label: String, tier: u8, confidence: f32, token_id: String) {
        if synthetic.len() > self.max_synthetic_key_len {
            self.max_synthetic_key_len = synthetic.len();
        }
        self.original_to_synthetic.insert(original.clone(), synthetic.clone());
        self.synthetic_keys.push(synthetic);
        self.original_values.push(original);
        self.pii_type_labels.push(pii_type_label);
        self.tiers.push(tier);
        self.confidences.push(confidence);
        self.token_ids.push(token_id);
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
                tracing::warn!(err = %e, "vault: AhoCorasick build failed");
                self.reverse_automaton = None;
            }
        }
    }

    /// Replace all original PII values with their synthetic equivalents.
    ///
    /// Intended for an outbound (request) path that performs vault-driven replacement
    /// without calling the PII pipeline tiers (e.g. a hypothetical "vault-only" mode
    /// where mappings from a previous session are reused directly).  In the current
    /// T3 standalone path the SLM sidecar already returns rewritten text, so this
    /// method is not called there — the vault is populated via `add_mapping` for the
    /// inbound reverse pass only.
    #[cfg_attr(not(test), allow(dead_code))]
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
        if any {
            tracing::info!(text_len = text.len(), "vault: synthetic reverse-replacement applied");
        }
        (result, any)
    }

    /// Snapshot the current vault state for persistence.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn to_records(&self) -> Vec<VaultRecord> {
        self.synthetic_keys
            .iter()
            .zip(self.original_values.iter())
            .zip(self.pii_type_labels.iter())
            .zip(self.tiers.iter())
            .zip(self.confidences.iter())
            .zip(self.token_ids.iter())
            .map(|(((((syn, orig), label), tier), conf), tid)| VaultRecord {
                original: orig.clone(),
                synthetic: syn.clone(),
                pii_type: PiiType::Custom(label.clone()),
                tier: *tier,
                confidence: *conf,
                token_id: tid.clone(),
                display_value: syn.clone(), // synthetic_keys stores display values
            })
            .collect()
    }

    /// Iterator over (original, synthetic, pii_type_label, tier, confidence) quints.
    pub fn quints(&self) -> impl Iterator<Item = (&str, &str, &str, u8, f32)> {
        self.original_values
            .iter()
            .zip(self.synthetic_keys.iter())
            .zip(self.pii_type_labels.iter())
            .zip(self.tiers.iter())
            .zip(self.confidences.iter())
            .map(|((((o, s), t), tier), conf)| (o.as_str(), s.as_str(), t.as_str(), *tier, *conf))
    }

    /// Iterator over (original, synthetic, pii_type_label) triples.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn triples(&self) -> impl Iterator<Item = (&str, &str, &str)> {
        self.original_values
            .iter()
            .zip(self.synthetic_keys.iter())
            .zip(self.pii_type_labels.iter())
            .map(|((o, s), t)| (o.as_str(), s.as_str(), t.as_str()))
    }

    pub fn is_empty(&self) -> bool {
        self.original_to_synthetic.is_empty()
    }

    /// Returns true if `value` is already a synthetic token in this vault.
    /// Used to prevent chaining: a synthetic should never become an original.
    pub fn is_synthetic(&self, value: &str) -> bool {
        self.synthetic_keys.iter().any(|s| s == value)
    }

    /// Number of stored mappings.
    pub fn mapping_count(&self) -> usize {
        self.original_to_synthetic.len()
    }

    /// Iterator over (original, synthetic) string pairs.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn pairs(&self) -> impl Iterator<Item = (&str, &str)> {
        self.original_values
            .iter()
            .zip(self.synthetic_keys.iter())
            .map(|(o, s)| (o.as_str(), s.as_str()))
    }

    /// Copy all three index HashMaps from `other` into `self`, skipping entries already present.
    fn merge_index_maps_from(&mut self, other: &PiiVault) {
        for (k, v) in &other.full_token_to_original {
            self.full_token_to_original.entry(k.clone()).or_insert_with(|| v.clone());
        }
        for (k, v) in &other.token_id_to_original {
            self.token_id_to_original.entry(k.clone()).or_insert_with(|| v.clone());
        }
        for (k, v) in &other.display_value_to_original {
            self.display_value_to_original.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }

    /// Returns the first 2 bytes of each synthetic key. Keys shorter than 2 bytes are skipped.
    /// Used by ReplacementBuffer for 2-byte trigger-prefix matching.
    pub fn synthetic_key_prefixes(&self) -> impl Iterator<Item = [u8; 2]> + '_ {
        self.synthetic_keys.iter().filter_map(|s| {
            let b = s.as_bytes();
            if b.len() >= 2 { Some([b[0], b[1]]) } else { None }
        })
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
    #[cfg_attr(not(test), allow(dead_code))]
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

        // Cache miss — load from storage or create fresh (outside the lock).
        let vault = Self::load_or_create(conv_id, store);
        let handle = Arc::new(RwLock::new(vault));
        // Double-check: another caller may have won the race while we loaded.
        let mut map = self.vaults.lock().unwrap();
        if let Some(entry) = map.get_mut(conv_id) {
            entry.last_accessed = Instant::now();
            return Arc::clone(&entry.handle);
        }
        map.insert(conv_id.to_string(), VaultEntry {
            handle: Arc::clone(&handle),
            last_accessed: Instant::now(),
        });
        handle
    }

    /// Load a vault from storage, or create a fresh empty one on miss or error.
    fn load_or_create(conv_id: &str, store: &Store) -> PiiVault {
        match store.load_vault(conv_id) {
            Ok(Some((seed, records))) => {
                tracing::info!(conv_id = %conv_id, mappings = records.len(), "vault: restored from storage");
                let vault_records: Vec<VaultRecord> = records
                    .into_iter()
                    .map(|r| VaultRecord {
                        original: r.original,
                        synthetic: r.synthetic,
                        pii_type: PiiType::Custom(r.pii_type),
                        tier: r.tier.unwrap_or(0),
                        confidence: r.confidence.unwrap_or(0.0),
                        token_id: r.token_id.unwrap_or_default(),
                        display_value: r.display_value.unwrap_or_default(),
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
        }
    }

    /// Move all mappings from the `from_key` vault into the `into_key` vault and
    /// remove `from_key` from the registry.
    ///
    /// If `into_key` doesn't exist yet, the `from_key` entry is simply re-keyed.
    /// If `from_key` doesn't exist this is a no-op.
    /// Used when a session_uuid fallback vault needs to be merged into the real conv_id
    /// vault once the conversation ID becomes known on a later turn.
    pub fn merge_into(&self, from_key: &str, into_key: &str, store: &Store) {
        let mut map = self.vaults.lock().unwrap();
        let from_entry = match map.remove(from_key) {
            Some(e) => e,
            None => return, // nothing to merge
        };
        if let Some(into_entry) = map.get(&into_key.to_string()) {
            // Both exist — drain from_key mappings into into_key vault.
            let from_vault = from_entry.handle.read().unwrap();
            let mut into_vault = into_entry.handle.write().unwrap();
            for (orig, syn, label, tier, conf) in from_vault.quints() {
                if into_vault.original_to_synthetic.contains_key(orig) {
                    continue;
                }
                into_vault.insert_mapping_raw(
                    orig.to_string(),
                    syn.to_string(),
                    label.to_string(),
                    tier,
                    conf,
                );
            }
            // Merge index maps (token_id/display_value lookups).
            into_vault.merge_index_maps_from(&from_vault);
            if !from_vault.is_empty() {
                into_vault.rebuild_automaton();
                tracing::info!(from_key = %from_key, into_key = %into_key, "vault: merged session_uuid vault into real conv_id vault");
            }
        } else {
            // into_key doesn't exist yet in the in-memory cache. Load any persisted
            // vault from storage so we don't clobber prior mappings for this conv_id.
            let persisted = Self::load_or_create(into_key, store);
            let merged_handle = if persisted.is_empty() {
                // No prior persisted vault — simply re-key the from_key handle.
                from_entry.handle
            } else {
                // Prior persisted vault exists — merge from_key mappings into it.
                let from_vault = from_entry.handle.read().unwrap();
                let mut into_vault = persisted;
                for (orig, syn, label, tier, conf) in from_vault.quints() {
                    if into_vault.original_to_synthetic.contains_key(orig) {
                        continue;
                    }
                    into_vault.insert_mapping_raw(
                        orig.to_string(),
                        syn.to_string(),
                        label.to_string(),
                        tier,
                        conf,
                    );
                }
                // Merge index maps.
                into_vault.merge_index_maps_from(&from_vault);
                if !from_vault.is_empty() {
                    into_vault.rebuild_automaton();
                }
                Arc::new(RwLock::new(into_vault))
            };
            map.insert(into_key.to_string(), VaultEntry {
                handle: merged_handle,
                last_accessed: Instant::now(),
            });
            tracing::info!(from_key = %from_key, into_key = %into_key, "vault: re-keyed session_uuid vault to real conv_id");
        }
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
        vault.add_mapping("john@acme.com".to_string(), "alice.brown@example.com".to_string(), &PiiType::Email, 1, 0.9f32);
        assert_eq!(vault.get_synthetic("john@acme.com"), Some("alice.brown@example.com"));
        assert_eq!(vault.get_synthetic("nobody@nowhere.com"), None);
    }

    #[test]
    fn test_idempotent_add() {
        let mut vault = PiiVault::new("test-conv-2");
        vault.add_mapping("a@b.com".to_string(), "x@example.com".to_string(), &PiiType::Email, 1, 0.9f32);
        vault.add_mapping("a@b.com".to_string(), "y@example.com".to_string(), &PiiType::Email, 1, 0.9f32);
        // Second add is ignored
        assert_eq!(vault.get_synthetic("a@b.com"), Some("x@example.com"));
    }

    #[test]
    fn test_replace_synthetics_round_trip() {
        let mut vault = PiiVault::new("test-conv-3");
        vault.add_mapping("John Smith".to_string(), "Alice Brown".to_string(), &PiiType::PersonName, 1, 0.9f32);
        vault.add_mapping("john@acme.com".to_string(), "alice@example.com".to_string(), &PiiType::Email, 1, 0.9f32);

        let text = "Contact Alice Brown at alice@example.com for details.";
        let (result, any) = vault.replace_synthetics(text);
        assert!(any);
        assert_eq!(result, "Contact John Smith at john@acme.com for details.");
    }

    #[test]
    fn test_replace_synthetics_longest_match() {
        let mut vault = PiiVault::new("test-conv-4");
        vault.add_mapping("Smith".to_string(), "Brown".to_string(), &PiiType::PersonName, 1, 0.9f32);
        vault.add_mapping("John Smith".to_string(), "Alice Brown".to_string(), &PiiType::PersonName, 1, 0.9f32);

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
            1,
            0.9f32,
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
            1,
            0.9f32,
        );
        vault.add_mapping(
            "123-45-6789".to_string(),
            "999-00-0001".to_string(),
            &PiiType::Ssn,
            1,
            0.9f32,
        );
        vault.add_mapping(
            "Alice Smith".to_string(),
            "Synthetic Name".to_string(),
            &PiiType::PersonName,
            1,
            0.9f32,
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
            1,
            0.9f32,
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
            1,
            0.9f32,
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
            1,
            0.9f32,
        );
        vault.add_mapping(
            "Bob Jones".to_string(),
            "Fake Person".to_string(),
            &PiiType::PersonName,
            1,
            0.9f32,
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
            tier: 1,
            confidence: 0.9,
            token_id: String::new(),
            display_value: String::new(),
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
            1,
            0.9f32,
        );

        let records = vault.to_records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].synthetic, "bob@example.com");
        assert_eq!(records[0].original, "alice@acme.com");
    }

    #[test]
    fn test_mapping_count() {
        let mut vault = PiiVault::new("test-conv-count");
        vault.add_mapping("a@a.com".to_string(), "x@x.com".to_string(), &PiiType::Email, 1, 0.9f32);
        vault.add_mapping("b@b.com".to_string(), "y@y.com".to_string(), &PiiType::Email, 1, 0.9f32);
        vault.add_mapping("c@c.com".to_string(), "z@z.com".to_string(), &PiiType::Email, 1, 0.9f32);
        assert_eq!(vault.mapping_count(), 3);
    }

    #[test]
    fn test_pairs_iter() {
        let mut vault = PiiVault::new("test-conv-pairs");
        vault.add_mapping(
            "orig-one".to_string(),
            "syn-one".to_string(),
            &PiiType::PersonName,
            1,
            0.9f32,
        );
        vault.add_mapping(
            "orig-two".to_string(),
            "syn-two".to_string(),
            &PiiType::PersonName,
            1,
            0.9f32,
        );

        let collected: Vec<(&str, &str)> = vault.pairs().collect();
        assert_eq!(collected.len(), 2);

        let has_one = collected.iter().any(|&(o, s)| o == "orig-one" && s == "syn-one");
        let has_two = collected.iter().any(|&(o, s)| o == "orig-two" && s == "syn-two");
        assert!(has_one, "pair (orig-one, syn-one) not found");
        assert!(has_two, "pair (orig-two, syn-two) not found");
    }

    #[test]
    fn test_synthetic_key_prefixes() {
        let mut vault = PiiVault::new("test-conv-prefixes");
        vault.add_mapping("orig-a".to_string(), "Alpha_token".to_string(), &PiiType::PersonName, 1, 0.9f32);
        vault.add_mapping("orig-b".to_string(), "Beta_token".to_string(), &PiiType::PersonName, 1, 0.9f32);
        vault.add_mapping("orig-c".to_string(), "Gamma_token".to_string(), &PiiType::PersonName, 1, 0.9f32);

        let prefixes: std::collections::HashSet<[u8; 2]> = vault.synthetic_key_prefixes().collect();
        assert!(prefixes.contains(&[b'A', b'l']), "missing 'Al' prefix: {prefixes:?}");
        assert!(prefixes.contains(&[b'B', b'e']), "missing 'Be' prefix: {prefixes:?}");
        assert!(prefixes.contains(&[b'G', b'a']), "missing 'Ga' prefix: {prefixes:?}");
        assert_eq!(prefixes.len(), 3, "expected exactly 3 distinct prefixes");
    }

    #[test]
    fn test_synthetic_key_prefixes_mixed_types() {
        let mut vault = PiiVault::new("test-conv-prefixes-mixed");
        // Email synthetic
        vault.add_mapping(
            "john@acme.com".to_string(),
            "alice.smith@example.com".to_string(),
            &PiiType::Email,
            1,
            0.9f32,
        );
        // IPv6 synthetic (fd prefix from gen_ipv6)
        vault.add_mapping(
            "2001:db8::1".to_string(),
            "fd1a2b:3c4d::1".to_string(),
            &PiiType::IpV6,
            1,
            0.9f32,
        );
        // Single-byte key should be skipped
        vault.add_mapping(
            "x".to_string(),
            "Y".to_string(),
            &PiiType::Custom("test".to_string()),
            1,
            0.9f32,
        );

        let prefixes: Vec<[u8; 2]> = vault.synthetic_key_prefixes().collect();
        // "alice.smith@example.com" -> [b'a', b'l']
        assert!(prefixes.contains(&[b'a', b'l']), "missing email prefix 'al': {prefixes:?}");
        // "fd1a2b:3c4d::1" -> [b'f', b'd']
        assert!(prefixes.contains(&[b'f', b'd']), "missing IPv6 prefix 'fd': {prefixes:?}");
        // Single-byte "Y" should have been skipped
        assert!(!prefixes.iter().any(|p| p[0] == b'Y'), "single-byte key should be skipped");
        assert_eq!(prefixes.len(), 2, "expected 2 prefixes (single-byte skipped)");
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
            1,
            0.9f32,
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
    fn test_deterministic_rng_different_conv_ids() {
        // Different conversation IDs must produce different RNG seeds so that
        // synthetic values don't collide across conversations.
        let v1 = PiiVault::new("conv-alpha");
        let v2 = PiiVault::new("conv-beta");
        let v3 = PiiVault::new("conv-gamma");

        assert_ne!(v1.rng_seed, v2.rng_seed, "conv-alpha and conv-beta share a seed");
        assert_ne!(v1.rng_seed, v3.rng_seed, "conv-alpha and conv-gamma share a seed");
        assert_ne!(v2.rng_seed, v3.rng_seed, "conv-beta and conv-gamma share a seed");
    }

    #[test]
    fn test_vault_registry_ttl_eviction() {
        // Use a very short TTL so we can trigger eviction without sleeping long.
        let registry = VaultRegistry::new(Duration::from_millis(50));

        // Populate two vaults.
        let _h1 = registry.get_or_create("evict-conv-1");
        let _h2 = registry.get_or_create("evict-conv-2");

        // Confirm both are present before eviction.
        {
            let map = registry.vaults.lock().unwrap();
            assert_eq!(map.len(), 2, "expected 2 vaults before eviction");
        }

        // Wait for the TTL to expire.
        std::thread::sleep(Duration::from_millis(100));

        registry.evict_expired();

        // Both vaults should have been evicted.
        {
            let map = registry.vaults.lock().unwrap();
            assert_eq!(map.len(), 0, "expected 0 vaults after TTL eviction");
        }
    }

    // ── 11.4 Vault confidence (quints + from_records) ─────────────────────────

    #[test]
    fn quints_returns_correct_five_tuples() {
        let mut vault = PiiVault::new("conv-quints");
        vault.add_mapping("alice@acme.com".to_string(), "bob@example.com".to_string(), &PiiType::Email, 1, 0.95);
        vault.add_mapping("John Doe".to_string(), "Jane Smith".to_string(), &PiiType::PersonName, 2, 0.72);

        let quints: Vec<_> = vault.quints().collect();
        assert_eq!(quints.len(), 2);

        let eq = quints.iter().find(|q| q.0 == "alice@acme.com").expect("email quint missing");
        assert_eq!(eq.1, "bob@example.com");
        assert_eq!(eq.2, "EMAIL");
        assert_eq!(eq.3, 1u8);
        assert!((eq.4 - 0.95).abs() < 1e-5, "email confidence: {}", eq.4);

        let nq = quints.iter().find(|q| q.0 == "John Doe").expect("name quint missing");
        assert_eq!(nq.3, 2u8);
        assert!((nq.4 - 0.72).abs() < 1e-5, "name confidence: {}", nq.4);
    }

    #[test]
    fn quints_empty_vault_returns_empty_iterator() {
        let vault = PiiVault::new("empty");
        assert_eq!(vault.quints().count(), 0);
    }

    /// Verify that the number of items produced by quints() always equals mapping_count().
    #[test]
    fn quints_count_equals_mapping_count() {
        let mut vault = PiiVault::new("conv-quints-count");
        for i in 0..7u32 {
            vault.add_mapping(
                format!("orig-{i}@test.com"),
                format!("synth-{i}@test.com"),
                &PiiType::Email,
                1,
                i as f32 / 7.0,
            );
        }
        assert_eq!(vault.quints().count(), vault.mapping_count());
        assert_eq!(vault.quints().count(), 7);
    }

    /// from_records with explicit confidence values restores them correctly.
    #[test]
    fn from_records_restores_confidence() {
        let records = vec![
            VaultRecord { original: "a@a.com".to_string(), synthetic: "x@x.com".to_string(), pii_type: PiiType::Email, tier: 1, confidence: 0.99, token_id: String::new(), display_value: String::new() },
            VaultRecord { original: "b@b.com".to_string(), synthetic: "y@y.com".to_string(), pii_type: PiiType::Email, tier: 2, confidence: 0.50, token_id: String::new(), display_value: String::new() },
        ];
        let vault = PiiVault::from_records("conv-fr-conf", 0, records);
        let quints: Vec<_> = vault.quints().collect();

        let qa = quints.iter().find(|q| q.0 == "a@a.com").unwrap();
        assert!((qa.4 - 0.99).abs() < 1e-5);

        let qb = quints.iter().find(|q| q.0 == "b@b.com").unwrap();
        assert!((qb.4 - 0.50).abs() < 1e-5);
    }

    /// from_records with zero confidence (legacy sentinel) must not be coerced.
    #[test]
    fn from_records_zero_confidence_preserved() {
        let records = vec![VaultRecord {
            original: "legacy@corp.com".to_string(),
            synthetic: "safe@example.com".to_string(),
            pii_type: PiiType::Email,
            tier: 0,
            confidence: 0.0,
            token_id: String::new(),
            display_value: String::new(),
        }];
        let vault = PiiVault::from_records("conv-zero-conf", 0, records);
        let quints: Vec<_> = vault.quints().collect();
        assert_eq!(quints.len(), 1);
        assert!((quints[0].4 - 0.0).abs() < 1e-9,
            "zero confidence must be preserved as exactly 0.0, got {}", quints[0].4);
        assert_eq!(quints[0].3, 0u8, "zero tier must be preserved");
    }

    /// Idempotent add_mapping does not create a duplicate confidence entry.
    #[test]
    fn idempotent_add_does_not_duplicate_confidence_entry() {
        let mut vault = PiiVault::new("conv-idem-conf");
        vault.add_mapping("a@a.com".to_string(), "x@x.com".to_string(), &PiiType::Email, 1, 0.9);
        vault.add_mapping("a@a.com".to_string(), "y@y.com".to_string(), &PiiType::Email, 1, 0.5);

        let quints: Vec<_> = vault.quints().collect();
        assert_eq!(quints.len(), 1, "idempotent add must not produce duplicate entries");
        // First mapping's confidence is preserved.
        assert!((quints[0].4 - 0.9).abs() < 1e-5);
    }

    // ── Group 1: Token ID and index structure tests ────────────────────────────

    #[test]
    fn generate_token_id_deterministic() {
        let t1 = generate_token_id("conv-abc", 0);
        let t2 = generate_token_id("conv-abc", 0);
        assert_eq!(t1, t2, "same inputs must produce same token_id");
        assert_eq!(t1.len(), 8, "token_id must be exactly 8 chars");
        assert!(t1.chars().all(|c| c.is_ascii_alphanumeric()), "all chars must be base62: {t1}");
    }

    #[test]
    fn generate_token_id_distinct() {
        let t0 = generate_token_id("conv-abc", 0);
        let t1 = generate_token_id("conv-abc", 1);
        assert_ne!(t0, t1, "different entity_index must produce different token_id");
        let t_other = generate_token_id("conv-xyz", 0);
        assert_ne!(t0, t_other, "different conversation_id must produce different token_id");
    }

    #[test]
    fn add_mapping_with_token_id_populates_all_maps() {
        let mut vault = PiiVault::new("test-add-with-tid");
        vault.add_mapping_with_token_id(
            "john@acme.com",
            "alice.brown@example.com",
            "a3f9b2c1",
            &PiiType::Email,
            1,
            1.0,
        );
        // full_token_to_original
        let full = r#"<pii id="a3f9b2c1">alice.brown@example.com</pii>"#;
        assert_eq!(vault.full_token_to_original.get(full).map(|s| s.as_str()), Some("john@acme.com"));
        // token_id_to_original
        assert_eq!(vault.token_id_to_original.get("a3f9b2c1").map(|s| s.as_str()), Some("john@acme.com"));
        // display_value_to_original
        assert_eq!(vault.display_value_to_original.get("alice.brown@example.com").map(|s| s.as_str()), Some("john@acme.com"));
        // mapping count incremented
        assert_eq!(vault.mapping_count(), 1);
    }

    #[test]
    fn get_by_token_id_hit_and_miss() {
        let mut vault = PiiVault::new("test-get-by-tid");
        vault.add_mapping_with_token_id("original@test.com", "synth@example.com", "tok1id00", &PiiType::Email, 1, 1.0);
        assert_eq!(vault.get_by_token_id("tok1id00"), Some("original@test.com"));
        assert_eq!(vault.get_by_token_id("zzzzzzzz"), None);
    }

    #[test]
    fn get_by_display_value_hit_and_miss() {
        let mut vault = PiiVault::new("test-get-by-dv");
        vault.add_mapping_with_token_id("Anne Nicole", "Maria Blinke", "a3f9b2c1", &PiiType::PersonName, 3, 1.0);
        assert_eq!(vault.get_by_display_value("Maria Blinke"), Some("Anne Nicole"));
        assert_eq!(vault.get_by_display_value("Unknown Person"), None);
    }

    #[test]
    fn from_records_populates_new_maps_from_persisted_token_id() {
        let records = vec![VaultRecord {
            original: "john@acme.com".to_string(),
            synthetic: "alice.brown@example.com".to_string(),
            pii_type: PiiType::Email,
            tier: 1,
            confidence: 1.0,
            token_id: "a3f9b2c1".to_string(),
            display_value: "alice.brown@example.com".to_string(),
        }];
        let vault = PiiVault::from_records("conv-fr-tid", 0, records);
        // Level 1 map
        let full = r#"<pii id="a3f9b2c1">alice.brown@example.com</pii>"#;
        assert_eq!(vault.full_token_to_original.get(full).map(|s| s.as_str()), Some("john@acme.com"));
        // Level 2 map
        assert_eq!(vault.get_by_token_id("a3f9b2c1"), Some("john@acme.com"));
        // Level 3 map
        assert_eq!(vault.get_by_display_value("alice.brown@example.com"), Some("john@acme.com"));
    }

    #[test]
    fn add_mapping_with_token_id_is_idempotent() {
        let mut vault = PiiVault::new("test-idem-tid");
        vault.add_mapping_with_token_id("orig@test.com", "synth@ex.com", "tid0001a", &PiiType::Email, 1, 1.0);
        vault.add_mapping_with_token_id("orig@test.com", "other@ex.com", "tid0002b", &PiiType::Email, 1, 1.0);
        assert_eq!(vault.mapping_count(), 1, "second call must be a no-op");
    }

    #[test]
    fn test_replace_synthetics_partial_overlap() {
        let mut vault = PiiVault::new("test-conv-overlap");
        vault.add_mapping(
            "short-original".to_string(),
            "AAA_TOKEN".to_string(),
            &PiiType::Custom("test".to_string()),
            1,
            0.9f32,
        );
        vault.add_mapping(
            "long-original".to_string(),
            "AAA_TOKEN_LONG".to_string(),
            &PiiType::Custom("test".to_string()),
            1,
            0.9f32,
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
