# Tasks: PII Detection and Bidirectional Replacement (Phase 2)

## 0. Scaffolding and Dependencies

- [ ] 0.1 Add `[pii]` section to `config.rs` with `PiiConfig` struct (enabled, mode, tier flags, NER config, SLM config, vault config, locale config)
- [ ] 0.2 Add new Cargo dependencies: `aho-corasick = "1.1"`, `regex = "1"`, `fancy-regex = "0.13"`, `fake = { version = "2.9", features = ["derive"] }`, `rand = "0.8"`, `reqwest = { version = "0.12", features = ["json", "stream"] }`
- [ ] 0.3 Add optional Cargo feature `ort-ner` gating `ort = { version = "2", features = ["download-binaries"] }` and `tokenizers = "0.19"` and `ndarray = "0.16"`
- [ ] 0.4 Create `src/pii/` directory with stub `mod.rs`
- [ ] 0.5 Create `src/models/` directory with stub `mod.rs`

## 1. PII Vault (`src/pii/vault.rs`)

- [ ] 1.1 Implement `PiiType` enum with all structured and unstructured types
- [ ] 1.2 Implement `PiiVault` struct: bidirectional HashMaps, AhoCorasick automaton, max key length, seeded RNG, conversation_id, created_at
- [ ] 1.3 Implement `PiiVault::new(conversation_id: &str) -> Self` — seeds RNG from sha256(conv_id)
- [ ] 1.4 Implement `PiiVault::add_mapping(&mut self, original: &str, synthetic: &str, pii_type: PiiType)` — adds to both maps, rebuilds AhoCorasick
- [ ] 1.5 Implement `PiiVault::get_synthetic(&self, original: &str) -> Option<&str>`
- [ ] 1.6 Implement `PiiVault::replace_originals(&self, text: &str) -> String` — uses forward HashMap for outbound
- [ ] 1.7 Implement `PiiVault::replace_synthetics(&self, text: &str) -> (String, bool)` — uses AhoCorasick for inbound; returns (replaced_text, any_match_found)
- [ ] 1.8 Implement `VaultRegistry`: `Arc<Mutex<HashMap<String, VaultHandle>>>` with TTL eviction
- [ ] 1.9 Implement `VaultRegistry::get_or_create(conv_id: &str) -> VaultHandle`
- [ ] 1.10 Implement `VaultRegistry::evict_expired()` — removes entries older than TTL
- [ ] 1.11 Unit tests: add 20 mappings, verify round-trip replace, verify AhoCorasick rebuild, verify deterministic RNG (same conv_id → same synthetic output)

## 2. Vault Persistence (`src/storage/mod.rs`)

- [ ] 2.1 Add vault serialization: `VaultEntry { type: "vault", mappings: Vec<(String, String, PiiType)>, rng_seed: u64 }` as NDJSON line
- [ ] 2.2 Implement `Store::save_vault(conv_id: &str, vault: &PiiVault) -> Result<()>` — appends/updates vault line in conversation NDJSON file
- [ ] 2.3 Implement `Store::load_vault(conv_id: &str) -> Result<Option<SavedVault>>` — reads vault line from conversation file
- [ ] 2.4 Update `VaultRegistry::get_or_create` to attempt `store.load_vault()` before creating a new empty vault
- [ ] 2.5 Unit tests: create vault, save, load, verify mappings preserved

## 3. Tier 1 Regex Detection (`src/pii/tier1.rs`)

- [ ] 3.1 Implement `PiiSpan { start, end, entity_type, confidence: 1.0, tier: 1 }` and `DetectionResult` types
- [ ] 3.2 Port Presidio patterns for: Email, US Phone, US SSN (with Luhn-equivalent validation for SSN format), Credit Card (with Luhn check), IPv4, IPv6
- [ ] 3.3 Port patterns for: OpenAI API key (`sk-...`), AWS access key (`AKIA...`), AWS secret, GitHub PAT (`ghp_...`, `github_pat_...`), generic Bearer token, SSH private key block, database connection string (postgres://, mysql://, mongodb://)
- [ ] 3.4 Implement `Tier1Detector::detect(text: &str, locale: &Locale) -> Vec<PiiSpan>`
- [ ] 3.5 Implement `Tier1Detector::detect_in_json_messages(messages: &[parser::Message], locale: &Locale) -> Vec<(usize, Vec<PiiSpan>)>` — returns (message_index, spans) for each message with detections
- [ ] 3.6 Use `fancy-regex` for patterns requiring negative lookahead (SSN); `regex` for all others
- [ ] 3.7 Unit tests: at least one positive and one negative test case per entity type; Luhn validation test

## 4. Synthetic Data Generation (`src/pii/synth.rs`)

- [ ] 4.1 Implement `SyntheticGenerator` with seeded `SmallRng`
- [ ] 4.2 Implement `generate(pii_type: &PiiType, original: &str, locale: &Locale) -> String` per type:
  - Email: `{fake_first}.{fake_last}@example.com`
  - Phone: `fake::faker::phone_number` with same country prefix as original
  - SSN: random digits `###-##-####` format
  - Credit card: random valid Luhn number (same card brand prefix)
  - IPv4: random RFC 1918 address (`10.x.x.x`)
  - IPv6: random `fd00::/8` address
  - API keys: same prefix pattern + random alphanumeric suffix (same length as original)
  - PersonName: `fake::faker::name::en_us::Name` (locale-aware)
  - OrgName: pick from a curated list ("Acme Corp", "Initech", "Globex", "Umbrella Corp", etc.)
  - Address: `fake::faker::address::en_us::StreetAddress`
  - DateOfBirth: shift by deterministic random offset (+7 to +97 days, seeded)
- [ ] 4.3 Implement `SyntheticGenerator::get_or_create(vault: &mut PiiVault, original: &str, pii_type: &PiiType) -> String` — idempotent: returns existing if already mapped
- [ ] 4.4 Unit tests: same seed → same output; different types produce different patterns

## 5. PII Pipeline Orchestrator (`src/pii/mod.rs`)

- [ ] 5.1 Implement `PiiPipeline { tier1: Tier1Detector, config: Arc<PiiConfig> }`
- [ ] 5.2 Implement `PiiPipeline::process_request(body: &[u8], vault: VaultHandle, provider: Provider) -> Result<Vec<u8>>`:
  - Parse JSON body using `serde_json`
  - Extract `messages` array
  - Run Tier 1 over all message `content` fields
  - (If Tier 2 enabled) run `Tier2Detector::detect()` on spans not found by Tier 1
  - (If Tier 3 enabled) escalate low-confidence Tier 2 spans to SLM sidecar
  - For each detected span: call `SyntheticGenerator::get_or_create()`, apply replacement
  - Rebuild JSON body with replaced content
  - Return modified body bytes
- [ ] 5.3 Implement `PiiPipeline::log_detections(spans: &[PiiSpan], provider: &str)` — tracing::info with entity type, tier, confidence (NO original text in logs — only `***` masked)
- [ ] 5.4 Unit tests: empty body, body with no PII, body with Tier 1 PII, multi-turn history

## 6. ReplacementBuffer (`src/pii/buffer.rs`)

- [ ] 6.1 Implement `ReplacementBuffer { vault: VaultHandle, buffer: String }`
- [ ] 6.2 Implement `process_delta(&mut self, incoming: &str) -> String`:
  - Append incoming to `self.buffer`
  - Run `vault.replace_synthetics()` over full buffer
  - Compute safe flush window: buffer length minus `max_synthetic_key_len`, OR full buffer if no trigger char found at tail
  - Return safe prefix; retain tail in `self.buffer`
- [ ] 6.3 Implement `flush_remaining(&mut self) -> String` — returns and clears all remaining buffer
- [ ] 6.4 Implement trigger char optimisation: `HashSet<char>` of first chars of all synthetic keys; skip buffering if tail doesn't start with any trigger char
- [ ] 6.5 Unit tests: single match spanning two chunks; no match (full flush); match at very end of stream; empty vault (zero latency passthrough)

## 7. intercept.rs Integration

- [ ] 7.1 Add `vault_registry: Arc<VaultRegistry>` and `pii_config: Arc<PiiConfig>` parameters to `intercept::run`
- [ ] 7.2 **Outbound path rewrite** (`handle_c2u`):
  - Remove immediate byte forwarding
  - Buffer complete request (headers + body)
  - After full body: call `PiiPipeline::process_request()`
  - Rebuild HTTP request with modified body and updated `Content-Length`
  - Forward complete modified request to upstream
  - Log sanitised content (not original) to dashboard/storage
- [ ] 7.3 **Inbound path rewrite** (`handle_u2c`):
  - Detect `text/event-stream` response
  - For SSE responses: extract text delta from SSE envelope, pass through `ReplacementBuffer`, re-wrap in SSE envelope, forward to client
  - For non-SSE responses: do NOT apply replacement (JSON body replacement not needed in Phase 2)
  - Call `store.save_vault(conv_id, &vault)` after each SSE stream completes
- [ ] 7.4 Update `proxy::run` and `proxy::network::run` to construct and pass `VaultRegistry`
- [ ] 7.5 Update `main.rs` `cmd_start` / `cmd_network_start` to construct `VaultRegistry` and pass to proxy
- [ ] 7.6 Integration test: pipe a request with "John Smith, john@acme.com" through the full pipeline; assert upstream sees synthetic replacement; assert client sees original values in SSE response

## 8. Tier 2 GLiNER ONNX (optional, `ort-ner` feature)

- [ ] 8.1 Implement `src/pii/tier2.rs` behind `#[cfg(feature = "ort-ner")]`
- [ ] 8.2 Implement `Tier2Detector::load(model_path: &Path) -> Result<Self>` — loads ONNX model via `ort::Session`
- [ ] 8.3 Implement `Tier2Detector::detect(text: &str, entity_labels: &[&str]) -> Result<Vec<PiiSpan>>`:
  - Tokenize text with `tokenizers::Tokenizer` (load tokenizer.json from model dir)
  - Format prompt: `entity_type_1 entity_type_2 ... << >> input_text` (GLiNER format)
  - Build input tensors: `input_ids`, `attention_mask` as `Array2<i64>`
  - Run `session.run()` with `ort::inputs![]` macro
  - Post-process span logits: sigmoid → threshold at `config.confidence_threshold` → decode to character offsets
  - Return `PiiSpan` list with `tier: 2` and `confidence` from model
- [ ] 8.4 Implement 500ms timeout: `tokio::time::timeout` wrapping inference; on timeout, log warning, return empty Vec (Tier 1 results used)
- [ ] 8.5 Implement batch processing: if messages array has multiple content fields, batch them into one inference call (up to 8 per batch)
- [ ] 8.6 Unit tests (with mock ONNX session or fixture): person name detection, org detection

## 9. Tier 3 SLM Sidecar (`src/pii/tier3.rs`)

- [ ] 9.1 Implement `SlmSidecar { client: reqwest::Client, endpoint: String, timeout: Duration }`
- [ ] 9.2 Implement `SlmSidecar::disambiguate(text: &str, candidates: &[PiiSpan]) -> Result<Vec<PiiSpan>>` — HTTP POST to llama-server `/v1/chat/completions` with structured prompt
- [ ] 9.3 Implement sidecar process management: `start_sidecar(llama_server_path: &Path, model_path: &Path) -> Result<Child>` and graceful shutdown on drop
- [ ] 9.4 Unit tests: mock HTTP server returning known JSON; timeout handling

## 10. Model Management (`src/models/`)

- [ ] 10.1 Implement `ModelRegistry` with catalog of supported models: GLiNER ONNX variants, Anonymizer SLM GGUF
- [ ] 10.2 Implement `models_install(model_name: &str, models_dir: &Path) -> Result<()>` — downloads with progress bar via `reqwest` streaming
- [ ] 10.3 Implement `models_list(models_dir: &Path) -> Result<Vec<InstalledModel>>`
- [ ] 10.4 Implement integrity check: sha256 of downloaded file against known hash in registry

## 11. Locale Support (`src/pii/locale.rs`)

- [ ] 11.1 Implement `Locale` enum: `EnUs`, `EnGb`, `DeDE`, `FrFr`, `InIN`, `KrKR`, `BrBR`
- [ ] 11.2 Implement locale-specific regex patterns for national IDs: UK NIN, German Steueridentifikationsnummer, French INSEE, Indian Aadhaar/PAN, Korean RRN/BRN, Brazilian CPF/CNPJ
- [ ] 11.3 Implement locale TOML pack loading from `config.locale_dir`: `[locale.en-US] ssn_pattern = "..."`, etc.
- [ ] 11.4 Implement `Tier1Detector::with_locale(locale: Locale)` — loads locale-specific recognizers in addition to universal ones

## 12. New CLI Commands (`src/main.rs`)

- [ ] 12.1 Add `TestPii { text: String, locale: Option<String> }` subcommand — runs Tier 1+2 pipeline and prints detections table
- [ ] 12.2 Add `Models { action: ModelsAction }` subcommand with `Install { name: String }` and `List` variants
- [ ] 12.3 Add `Benchmark { locale: Option<String>, tier: Option<String>, report: Option<String> }` subcommand stub (full AI4Privacy benchmark deferred to later iteration)
- [ ] 12.4 Update `start` command: add `--pii` / `--pii-llm` flags that override config

## 13. Dashboard PII Panel

- [ ] 13.1 Add `WsEvent::PiiDetected { conversation_id, message_index, entity_type, original_masked: String, synthetic: String, tier, confidence }` to `dashboard/mod.rs`
- [ ] 13.2 Add `GET /api/conversations/:id/vault` REST endpoint — returns vault mappings for a conversation (with originals masked as `***type***`)
- [ ] 13.3 Update `index.html` / `app.js`: add collapsible PII detection panel per message showing diff view and vault table
- [ ] 13.4 Add visual indicator on conversation list entry when PII was detected in that conversation

## 14. Configuration Extension

- [ ] 14.1 Add `PiiConfig` struct to `config.rs` with all fields from Task 3 spec `[pii]` section
- [ ] 14.2 Implement `PiiConfig::default()` — all features off, sensible thresholds
- [ ] 14.3 Validate config on load: if `pii.mode = "replace"` and `pii.tiers.ner = true` but model not found, warn and disable Tier 2 gracefully

## 15. Integration Tests

- [ ] 15.1 `tests/integration/pii_roundtrip_test.rs` — full round-trip: request with known PII → assert modified body sent upstream → mock SSE response with synthetic tokens → assert client receives original PII in SSE text
- [ ] 15.2 `tests/integration/vault_persistence_test.rs` — vault save/load across mock "proxy restart"
- [ ] 15.3 `tests/integration/multiturn_consistency_test.rs` — 5-turn conversation; same PII in turn 1 → same synthetic in turns 2-5
- [ ] 15.4 `tests/integration/passthrough_no_pii_test.rs` — request with zero PII → body forwarded unchanged → no vault entries created

## 16. Documentation

- [ ] 16.1 Update `config.example.toml` with full `[pii]` section
- [ ] 16.2 Update CLI `--help` descriptions for new subcommands
- [ ] 16.3 Add `docs/PII-SETUP.md` with quick-start guide, model installation, test-pii usage
