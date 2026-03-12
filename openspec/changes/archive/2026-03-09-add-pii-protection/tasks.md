# Tasks: PII Detection and Bidirectional Replacement (Phase 2)

## 0. Scaffolding and Dependencies

- [x] 0.1 Add `[pii]` section to `config.rs` with `PiiConfig` struct (enabled, mode, tier flags, NER config, SLM config, vault config, locale config)
- [x] 0.2 Add new Cargo dependencies: `aho-corasick = "1.1"`, `regex = "1"`, `fancy-regex = "0.13"`, `fake = { version = "2.9", features = ["derive"] }`, `rand = "0.8"`, `reqwest = { version = "0.12", features = ["json", "stream"] }`
- [x] 0.3 Add optional Cargo feature `ort-ner` gating `ort = { version = "2", features = ["download-binaries"] }` and `tokenizers = "0.19"` and `ndarray = "0.16"`
- [x] 0.4 Create `src/pii/` directory with stub `mod.rs`
- [x] 0.5 Create `src/models/` directory with stub `mod.rs`

## 1. PII Vault (`src/pii/vault.rs`)

- [x] 1.1 Implement `PiiType` enum with all structured and unstructured types
- [x] 1.2 Implement `PiiVault` struct: bidirectional HashMaps, AhoCorasick automaton, max key length, seeded RNG, conversation_id, created_at
- [x] 1.3 Implement `PiiVault::new(conversation_id: &str) -> Self` — seeds RNG from sha256(conv_id)
- [x] 1.4 Implement `PiiVault::add_mapping(&mut self, original: &str, synthetic: &str, pii_type: PiiType)` — adds to both maps, rebuilds AhoCorasick
- [x] 1.5 Implement `PiiVault::get_synthetic(&self, original: &str) -> Option<&str>`
- [x] 1.6 Implement `PiiVault::replace_originals(&self, text: &str) -> String` — uses forward HashMap for outbound
- [x] 1.7 Implement `PiiVault::replace_synthetics(&self, text: &str) -> (String, bool)` — uses AhoCorasick for inbound; returns (replaced_text, any_match_found)
- [x] 1.8 Implement `VaultRegistry`: `Arc<Mutex<HashMap<String, VaultHandle>>>` with TTL eviction
- [x] 1.9 Implement `VaultRegistry::get_or_create(conv_id: &str) -> VaultHandle`
- [x] 1.10 Implement `VaultRegistry::evict_expired()` — removes entries older than TTL
- [x] 1.11 Unit tests: add 20 mappings, verify round-trip replace, verify AhoCorasick rebuild, verify deterministic RNG (same conv_id → same synthetic output)

## 2. Vault Persistence (`src/storage/mod.rs`)

- [x] 2.1 Add vault serialization: `VaultEntry { type: "vault", mappings: Vec<(String, String, PiiType)>, rng_seed: u64 }` as NDJSON line
- [x] 2.2 Implement `Store::save_vault(conv_id: &str, vault: &PiiVault) -> Result<()>` — appends/updates vault line in conversation NDJSON file
- [x] 2.3 Implement `Store::load_vault(conv_id: &str) -> Result<Option<SavedVault>>` — reads vault line from conversation file
- [x] 2.4 Update `VaultRegistry::get_or_create` to attempt `store.load_vault()` before creating a new empty vault
- [x] 2.5 Unit tests: create vault, save, load, verify mappings preserved

## 3. Tier 1 Regex Detection (`src/pii/tier1.rs`)

- [x] 3.1 Implement `PiiSpan { start, end, entity_type, confidence: 1.0, tier: 1 }` and `DetectionResult` types
- [x] 3.2 Port Presidio patterns for: Email, US Phone, US SSN (with Luhn-equivalent validation for SSN format), Credit Card (with Luhn check), IPv4, IPv6
- [x] 3.3 Port patterns for: OpenAI API key (`sk-...`), AWS access key (`AKIA...`), AWS secret, GitHub PAT (`ghp_...`, `github_pat_...`), generic Bearer token, SSH private key block, database connection string (postgres://, mysql://, mongodb://)
- [x] 3.4 Implement `Tier1Detector::detect(text: &str, locale: &Locale) -> Vec<PiiSpan>`
- [x] 3.5 Implement `Tier1Detector::detect_in_json_messages` (via `replace_in_text` + pipeline)
- [x] 3.6 Use `fancy-regex` for all patterns (look-behind/look-ahead support); `find_iter` returns `Result<Match>` handled via `.filter_map(|r| r.ok())`
- [x] 3.7 Unit tests: at least one positive and one negative test case per entity type; Luhn validation test

## 4. Synthetic Data Generation (`src/pii/synth.rs`)

- [x] 4.1 Implement `SyntheticGenerator` with seeded `SmallRng`
- [x] 4.2 Implement `generate(pii_type: &PiiType, original: &str, locale: &Locale) -> String` per type (Email, Phone, SSN, CreditCard, IPv4, IPv6, API keys, PersonName, OrgName, Address, DateOfBirth)
- [x] 4.3 Implement `SyntheticGenerator::get_or_create(vault: &mut PiiVault, original: &str, pii_type: &PiiType) -> String` — idempotent
- [x] 4.4 Unit tests: same seed → same output; different types produce different patterns

## 5. PII Pipeline Orchestrator (`src/pii/mod.rs`)

- [x] 5.1 Implement `PiiPipeline { tier1: Tier1Detector, config: Arc<PiiConfig> }`
- [x] 5.2 Implement `PiiPipeline::process_request_body(body: &[u8], vault: &mut PiiVault, provider: Provider, locale: &Locale) -> Option<Vec<u8>>`
- [x] 5.3 Implement `PiiPipeline::log_detections(spans: &[PiiSpan], provider: &str)` — tracing::info with entity type, tier, confidence (NO original text in logs)
- [x] 5.4 Unit tests: empty body, body with no PII, body with Tier 1 PII, multi-turn history

## 6. ReplacementBuffer (`src/pii/buffer.rs`)

- [x] 6.1 Implement `ReplacementBuffer { vault: VaultHandle, buffer: String }`
- [x] 6.2 Implement `process_delta(&mut self, incoming: &str) -> String`
- [x] 6.3 Implement `flush_remaining(&mut self) -> String`
- [x] 6.4 Implement trigger char optimisation: `HashSet<char>` of first chars of all synthetic keys
- [x] 6.5 Unit tests: single match spanning two chunks; no match (full flush); match at very end of stream; empty vault (zero latency passthrough)

## 7. intercept.rs Integration

- [x] 7.1 Add `pii_ctx: PiiCtx` parameter threading through `intercept::run`
- [x] 7.2 **Outbound path rewrite** (`handle_c2u`): buffer request body, call `process_request_body`, rebuild with updated `Content-Length`, forward modified request
- [x] 7.3 **Inbound path rewrite** (`handle_u2c`): SSE text delta → `ReplacementBuffer` → re-wrap in SSE, save vault on `[DONE]`
- [x] 7.4 Update `proxy::run` and `proxy::network::run` to accept and thread `PiiCtx`
- [x] 7.5 Update `main.rs` `cmd_start` / `cmd_network_start` to construct `PiiCtx` and pass to proxy
- [x] 7.6 Integration test: pipe a request with "John Smith, john@acme.com" through the full pipeline (covered by section 15)

## 8. Tier 2 GLiNER ONNX (optional, `ort-ner` feature)

- [x] 8.1 `src/pii/tier2.rs` exists with `#[cfg(feature = "ort-ner")]` stub
- [x] 8.2 Implement `Tier2Detector::load` — loads ONNX model via `ort::Session`
- [x] 8.3 Implement `Tier2Detector::detect` with tokenizer + tensor pipeline
- [x] 8.4 500ms timeout wrapping inference
- [x] 8.5 Batch processing (BATCH_SIZE=8)
- [x] 8.6 Unit tests (load error, sigmoid helper)

## 9. Tier 3 SLM Sidecar (`src/pii/tier3.rs`)

- [x] 9.1 `src/pii/tier3.rs` stub exists
- [x] 9.2 `SidecarProcess`: spawn llama-server subprocess, kill on Drop
- [x] 9.3 `SlmSidecar`: reqwest HTTP client, `/v1/chat/completions`, fail-open on timeout/error
- [x] 9.4 Unit tests: constructor, timeout, mock TCP server subset confirmation

## 10. Model Management (`src/models/`)

- [x] 10.1 `ModelRegistry` with catalog of supported models exists
- [x] 10.2 `models::install(name, models_dir)` implemented with `reqwest` streaming + progress
- [x] 10.3 `models::list_installed(models_dir)` implemented
- [x] 10.4 Integrity check (sha256) implemented

## 11. Locale Support (`src/pii/locale.rs` + `tier1.rs`)

- [x] 11.1 Implement `Locale` enum: `EnUs`, `EnGb`, `DeDe`, `FrFr`, `InIn`, `KoKr`, `BrBr`
- [x] 11.2 Implement locale-specific regex patterns for national IDs: UK NIN, German Steueridentifikationsnummer, French INSEE, Indian Aadhaar/PAN, Korean RRN/BRN, Brazilian CPF/CNPJ
- [x] 11.3 Implement locale pattern loading (hardcoded in `tier1.rs::locale_patterns()`)
- [x] 11.4 Wire `locale_patterns()` into `Tier1Detector::detect()` (currently ignores `_locale`)

## 12. New CLI Commands (`src/main.rs`)

- [x] 12.1 `TestPii { text, locale, format }` subcommand — runs Tier 1 pipeline and prints table or JSON
- [x] 12.2 `Models { action: ModelsAction }` subcommand with `Install` and `List` variants
- [x] 12.3 `Benchmark { tier }` subcommand with built-in fixtures
- [x] 12.4 `Start` and `NetworkStart` commands: `--pii` / `--pii-llm` flags that override config

## 13. Dashboard PII Panel

- [x] 13.1 `WsEvent::PiiDetected { conversation_id, entity_type, original_masked, synthetic, tier, confidence }` added to `dashboard/mod.rs`
- [x] 13.2 `GET /api/conversations/:id/vault` REST endpoint returns vault mappings (originals masked as `[TYPE]`)
- [x] 13.3 Update `index.html` / `app.js`: collapsible PII panel per conversation, vault table, diff view
- [x] 13.4 Visual indicator on conversation list entry when PII detected

## 14. Configuration Extension

- [x] 14.1 `PiiConfig` struct in `config.rs` with all fields
- [x] 14.2 `PiiConfig::default()` — all features off, sensible thresholds
- [x] 14.3 Validate config on load: if `pii.mode = "replace"` and `pii.tiers.ner = true` but model not found, warn and disable Tier 2 gracefully

## 15. Integration Tests

- [x] 15.1 `tests/integration/pii_roundtrip_test.rs`
- [x] 15.2 `tests/integration/vault_persistence_test.rs`
- [x] 15.3 `tests/integration/multiturn_consistency_test.rs`
- [x] 15.4 `tests/integration/passthrough_no_pii_test.rs`

## 16. Documentation

- [x] 16.1 `config.example.toml` updated with full `[pii]` section
- [x] 16.2 CLI `--help` descriptions populated for all new subcommands
- [x] 16.3 `docs/PII-SETUP.md` added with quick-start guide, model installation, test-pii usage
