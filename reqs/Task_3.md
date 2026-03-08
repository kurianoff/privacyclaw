# TASK: claudovka Phase 2 — PII Detection and Bidirectional Replacement

## Context

claudovka Phase 1 is complete: a transparent MITM/reverse proxy intercepts traffic between AI coding agents (Claude Code CLI, VS Code Extension) and commercial LLM APIs (Anthropic, OpenAI). Traffic is captured and displayed in a real-time dashboard. No modification of traffic occurs yet.

Phase 2 adds the core privacy feature: detect PII/PHI/secrets in outbound requests, replace with synthetic data, and reverse the replacement in inbound responses.

## Key Architectural Insight

LLM API requests are ALWAYS non-streaming. The client sends a complete JSON body with all messages in one POST request. This means:

- **Outbound (user -> LLM)**: Full request body available. No streaming challenge. We can parse, detect, replace PII, store mappings, and forward — all before a single byte goes to the LLM.
- **Inbound (LLM -> user)**: SSE streaming response. But the task is simpler — only reverse replacement using known vault mappings. No detection needed on this path.

## Architecture

```
OUTBOUND PATH (user -> LLM):
  Full request body
       |
       v
  [Tier 1: Regex]  ----> emails, phones, SSNs, API keys, IPs, credit cards
       |
       v
  [Tier 2: GLiNER PII (ONNX)]  ----> names, orgs, locations, dates, IDs
       |
       v
  [Tier 3: Anonymizer SLM 1.7B (sidecar)]  ----> context-dependent disambiguation
       |                                           + synthetic replacement generation
       v
  PII Vault: store {original -> synthetic} mappings
       |
       v
  Forward sanitized request to LLM API


INBOUND PATH (LLM -> user):
  SSE stream chunks arrive
       |
       v
  [Aho-Corasick automaton built from vault synthetic keys]
       |
       v
  Sensitive-start detector: does current byte match
  any prefix of any vault key?
       |
       +-- NO:  flush chunk to client immediately (zero latency)
       |
       +-- YES: buffer until match confirmed or rejected
                |
                +-- MATCH: replace synthetic -> original, flush
                +-- NO MATCH: flush buffered bytes, resume
       |
       v
  Client receives restored response with original PII
```

## Functional Requirements

### 1. PII Vault

In-memory bidirectional HashMap per conversation session.

```rust
struct PiiVault {
    // Forward: original -> synthetic (used on outbound)
    original_to_synthetic: HashMap<String, String>,
    // Reverse: synthetic -> original (used on inbound)
    synthetic_to_original: HashMap<String, String>,
    // Aho-Corasick automaton rebuilt when vault changes
    reverse_automaton: AhoCorasick,
    // Max key length (for buffer sizing)
    max_synthetic_key_len: usize,
    // Session metadata
    conversation_id: String,
    created_at: DateTime<Utc>,
}
```

Requirements:
- Thread-safe (wrapped in `Arc<RwLock<>>>`).
- Consistent across multi-turn conversations: if "John Smith" maps to "Alice Brown" in turn 1, same mapping in turn 5.
- New PII found in subsequent turns gets added to the vault incrementally.
- Vault persists to SQLite for crash recovery, keyed by conversation_id.
- TTL cleanup: configurable, default 24 hours.
- Aho-Corasick automaton is rebuilt each time a new mapping is added (cheap operation for <100 keys).

### 2. Outbound Path: Detection and Replacement

When a request arrives at the proxy:

### Step 0: Port Presidio Patterns
- Extract regex patterns and validators from Presidio's country_specific recognizers.
- Convert to TOML locale pack format.
- Start with: en-US, en-GB, de-DE, fr-FR, in-IN, kr-KR, br-BR.
- Each locale pack: entity definitions with regex, context words, checksum functions.
- **Verify**: `claudovka test-pii --locale in-IN "My Aadhaar is 1234 5678 9012"` detects correctly.

**Step 1: Parse request body.**
Extract the `messages` array from the JSON body. Each message has `role` and `content`. Process the `content` of all messages (system, user, assistant from history).

**Step 2: Tier 1 — Regex detection.**
Run in-process, <2ms. Detect structured PII:
- Email addresses
- Phone numbers (international formats)
- SSN / national IDs (configurable by locale)
- Credit card numbers (with Luhn checksum validation)
- IP addresses (v4 and v6)
- API keys / tokens (common patterns: `sk-`, `ghp_`, `AKIA`, Bearer tokens)
- AWS access keys, GCP service account patterns
- SSH private key blocks
- Database connection strings
- URLs with embedded credentials

For each detection: generate a synthetic replacement of the same type (fake email for email, fake phone for phone). Store mapping in vault.

**Step 3: Tier 2 — GLiNER PII NER detection.**
Run via ONNX Runtime in-process (using `ort` crate). ~20-50ms per message.

Entity types to detect (passed as zero-shot labels):
- Person names (first, last, full)
- Organization names
- Physical addresses / locations
- Dates of birth
- Medical record numbers
- Passport / driver's license numbers
- Financial account numbers
- Any entity not caught by Tier 1 regex

For each detection: if the entity was already caught by Tier 1, skip. Otherwise, generate synthetic replacement using Faker-style logic (same type, same format, culturally appropriate). Store in vault.

**Step 4: Tier 3 — Anonymizer SLM (optional, for ambiguous cases).**
Run as sidecar process (llama-server on localhost). ~100-500ms.

Only invoked when Tier 2 returns entities with low confidence, or for known-ambiguous patterns:
- Common words that might be names ("Jordan", "Chase", "Grace")
- Organization names vs. common words ("Apple", "Amazon")
- Dates that might be meaningful vs. incidental

The SLM receives the text with Tier 1+2 detections marked, and decides:
- Is this actually PII in this context?
- What is a semantically appropriate synthetic replacement that preserves meaning?

SLM responses update the vault.

**Step 5: Apply replacements.**
Replace all detected PII in the request body with synthetic values from vault. Reconstruct the JSON. Forward to upstream LLM API.

**Important**: System prompts and assistant messages from conversation history also go through detection. The LLM must see a consistent world where all PII has been replaced throughout the entire conversation.

### 3. Inbound Path: Streaming Reverse Replacement

When SSE response chunks arrive from the LLM:

**The core algorithm uses a prefix-sensitive buffer.**

Build a `HashSet<u8>` of first bytes of all synthetic keys in vault. This is the "trigger set."

For each incoming SSE chunk, extract the text delta from the JSON envelope:
- Anthropic: `{"type":"content_block_delta","delta":{"type":"text_delta","text":"..."}}`
- OpenAI: `{"choices":[{"delta":{"content":"..."}}]}`

Process the extracted text through the replacement buffer:

```rust
struct ReplacementBuffer {
    vault: Arc<RwLock<PiiVault>>,
    buffer: String,
    trigger_chars: HashSet<u8>,  // First chars of all synthetic keys
}

impl ReplacementBuffer {
    fn process_text(&mut self, incoming: &str) -> String {
        self.buffer.push_str(incoming);

        let vault = self.vault.read();
        let replaced = vault.reverse_automaton.replace_all(
            &self.buffer,
            &vault.original_values()
        );

        // Safe zone: everything except trailing max_key_len chars
        let safe_len = replaced.len()
            .saturating_sub(vault.max_synthetic_key_len);

        // But: only buffer if the tail starts with a trigger char
        // If tail doesn't start with any trigger char, safe to flush all
        let actual_safe = if safe_len < replaced.len() {
            let tail = &replaced[safe_len..];
            if self.could_be_prefix_of_any_key(tail, &vault) {
                safe_len
            } else {
                replaced.len() // flush everything
            }
        } else {
            safe_len
        };

        let to_flush = replaced[..actual_safe].to_string();
        self.buffer = replaced[actual_safe..].to_string();
        to_flush
    }

    fn flush_remaining(&mut self) -> String {
        std::mem::take(&mut self.buffer)
    }
}
```

After processing, re-wrap the modified text back into the SSE JSON envelope and forward to client.

On stream end (`[DONE]` event or `message_stop`): flush remaining buffer.

### 4. Synthetic Data Generation

For replacements, generate data that:
- Preserves the TYPE (name for name, email for email)
- Preserves FORMAT (same length approximately, same locale)
- Is culturally consistent (Russian name replaced with Russian name, not "John Smith")
- Is DETERMINISTIC per vault — same input always gets same output within a session

Use a seeded PRNG per conversation for determinism. Replacement strategies by type:

| PII Type | Replacement Strategy |
|----------|---------------------|
| Person name | Faker name, same gender/culture if detectable |
| Email | `{synthetic_first}.{synthetic_last}@example.com` |
| Phone | Faker phone, same country code |
| Address | Faker address, same country |
| SSN / ID | Random digits, same format |
| Date of birth | Shift by consistent random offset (e.g., +47 days) |
| Credit card | Faker CC number (valid Luhn) |
| API key | Random string, same prefix pattern |
| Organization | "Acme Corp", "Initech", etc. from a list |
| IP address | Random private range IP |
| URL with creds | Same structure, synthetic creds |

### 5. Configuration

Add to `config.toml`:

```toml
[pii]
enabled = true
mode = "replace"  # "replace" | "detect-only" | "off"

[pii.tiers]
regex = true       # Tier 1: always on, <2ms
ner = true         # Tier 2: GLiNER ONNX, ~20-50ms
llm = false        # Tier 3: Anonymizer SLM sidecar, ~100-500ms (opt-in)

[pii.ner]
model = "gliner-pii-base-v1.0"           # ONNX model name
model_path = "~/.config/claudovka/models/" # Where ONNX models are stored
quantization = "uint8"                     # fp16 | uint8
confidence_threshold = 0.7                 # Below this -> escalate to Tier 3 if enabled

[pii.llm]
endpoint = "http://127.0.0.1:8090"  # llama-server address
model = "Anonymizer-1.7B-Q4_K_M"    # GGUF model
timeout_ms = 2000                     # Max wait for LLM sidecar

[pii.vault]
persistence = true
ttl_hours = 24
max_entries_per_session = 500

[pii.locale]
default = "en-US"
# Additional locale packs loaded from locale_dir
locale_dir = "~/.config/claudovka/locales/"
```

### 6. CLI Changes

```bash
# Download and install PII models
claudovka models install gliner-pii-base    # Downloads ONNX model (~200MB)
claudovka models install anonymizer-slm-1.7b # Downloads GGUF model (~1.5GB)
claudovka models list                        # Show installed models

# Start with PII protection enabled
claudovka start --pii                        # Tiers 1+2
claudovka start --pii --llm                  # Tiers 1+2+3 (starts llama-server sidecar)

# Test PII detection on a sample text
claudovka test-pii "My name is John Smith, email john@acme.com, SSN 123-45-6789"
# Output:
#   Detected 3 PII entities:
#     [PERSON] "John Smith" -> "Alice Brown" (Tier 2, confidence: 0.94)
#     [EMAIL] "john@acme.com" -> "alice.brown@example.com" (Tier 1, regex)
#     [SSN] "123-45-6789" -> "987-65-4321" (Tier 1, regex)
```

## Edge Model Recommendation

**Tier 2: GLiNER PII Base v1.0** (Knowledgator/Wordcab)
- Architecture: Bidirectional transformer (~110M params)
- Format: ONNX (FP16 + UINT8 quantized available)
- Inference: 2-20ms MacBook, 20-75ms Raspberry Pi
- Capabilities: Zero-shot, 60+ PII categories, no retraining needed
- Integration: `ort` crate (Rust ONNX Runtime bindings)
- Why: Fastest accurate NER model available. Encoder-based (single forward pass), not generative. Purpose-built for PII.

**Tier 3: Anonymizer SLM 1.7B** (Eternis AI, based on Qwen3)
- Architecture: Generative decoder (1.7B params)
- Format: GGUF Q4_K_M (~1.5GB)
- Inference: TTFT <250ms on Apple Silicon, 50-200ms per replacement
- Capabilities: Context-aware detection + semantic synthetic replacement in one pass
- Integration: llama-server sidecar, HTTP API on localhost
- Why: Only model that does detection AND culturally-appropriate replacement. Distinguishes private persons from public figures.

## Project Structure Changes

```
src/
  pii/
    mod.rs              # PII pipeline orchestrator (Tier 1 -> 2 -> 3)
    regex.rs            # Tier 1: regex patterns for structured PII
    ner.rs              # Tier 2: GLiNER ONNX inference via ort crate
    llm_sidecar.rs      # Tier 3: HTTP client to llama-server
    vault.rs            # PII vault: mappings, Aho-Corasick, persistence
    replacement.rs      # Synthetic data generation (Faker-style)
    buffer.rs           # Streaming reverse replacement buffer
    locale.rs           # Locale-specific patterns and generators
  models/
    mod.rs              # Model download, installation, version management
    registry.rs         # Available models catalog
```

## Implementation Order

### Step 1: PII Vault
- Implement `PiiVault` struct with bidirectional HashMap.
- Implement Aho-Corasick rebuild on mutation.
- SQLite persistence.
- Unit tests for mapping consistency, rebuild, serialization.
- **Verify**: Create vault, add 20 mappings, serialize to SQLite, reload, verify identical.

### Step 2: Tier 1 Regex Detection
- Implement regex patterns for all structured PII types.
- Implement synthetic replacement generators per type.
- Wire into outbound request pipeline: parse JSON body -> detect -> replace -> rebuild JSON.
- **Verify**: `claudovka test-pii "email me at john@acme.com, my SSN is 123-45-6789"` correctly detects and replaces.

### Step 3: Outbound Pipeline Integration
- Insert PII processing between request receipt and upstream forwarding.
- Parse Anthropic/OpenAI request body formats.
- Process all messages in the array (system + history + current).
- Rebuild JSON with replaced content.
- Forward to upstream.
- **Verify**: Make a Claude Code request through proxy. Check that the request arriving at Anthropic contains synthetic data (inspect via dashboard).

### Step 4: Inbound Reverse Replacement Buffer
- Implement `ReplacementBuffer` with prefix-sensitive buffering.
- Parse SSE text deltas from Anthropic/OpenAI response format.
- Replace synthetic -> original using vault automaton.
- Re-wrap into SSE envelope and forward to client.
- **Verify**: Send a request containing "John Smith", see it replaced with "Alice Brown" going to LLM, see "Alice Brown" in LLM response replaced back to "John Smith" arriving at client. Full round-trip.

### Step 5: Tier 2 GLiNER Integration
- Implement ONNX model loading via `ort` crate.
- Implement model download/install CLI commands.
- Implement inference: text + entity labels -> detected spans.
- Wire into outbound pipeline after Tier 1 (skip entities already found by regex).
- **Verify**: `claudovka test-pii "Please tell Maria Johnson at 42 Oak Street about the project"` detects person name and address that regex would miss.

### Step 6: Tier 3 Anonymizer SLM (optional)
- Implement llama-server sidecar management (start/stop).
- Implement HTTP client for sidecar inference.
- Wire as optional escalation for low-confidence Tier 2 detections.
- **Verify**: Ambiguous case like "Jordan called about the Amazon project" — SLM disambiguates "Jordan" (person) and "Amazon" (org vs. river vs. company).

### Step 7: Multi-turn Consistency
- Vault persists across turns in the same conversation.
- New messages in existing conversations load existing vault.
- New PII in subsequent turns gets added incrementally.
- **Verify**: Turn 1 mentions "John Smith". Turn 3 mentions "John" (short form). Both map to same synthetic replacement.

### Step 8: Dashboard Integration
- Dashboard shows PII detections per conversation.
- Visual diff: original text vs. sanitized text sent to LLM.
- Vault contents viewable per conversation.
- **Verify**: Open dashboard, see which PII was detected, what it was replaced with, and the full mapping table.

## Success Criteria

1. A Claude Code session through claudovka sends zero real PII to Anthropic's servers.
2. The LLM response is indistinguishable from a direct response — all synthetic names/data are seamlessly replaced back to originals.
3. Tier 1+2 adds <100ms total latency to outbound requests.
4. Inbound streaming replacement adds <5ms latency per SSE chunk (effectively zero perceived delay due to prefix-triggered buffering).
5. Multi-turn conversations maintain consistent PII mappings.
6. `claudovka test-pii` provides a fast way to verify detection quality on any text.
7. Works without Tier 3 (LLM sidecar) for users who don't want to run a local model — Tier 1+2 covers 90%+ of PII.

## Data Sources

### Taxonomy and Regex Patterns: Microsoft Presidio
Port Presidio's recognizer definitions into claudovka's TOML locale packs.
Do NOT use Presidio as a library (it's Python). Extract and reimplement:
- Regex patterns per entity type per country
- Context words that boost detection confidence
- Checksum validators (Luhn for credit cards, SSN validation, etc.)
- Country-specific recognizers (Korean RRN/BRN, Indian PAN/Aadhaar, 
  Brazilian CPF/CNPJ, EU IBAN/VAT, etc.)

Source: https://microsoft.github.io/presidio/supported_entities/
Source: https://github.com/microsoft/presidio (predefined_recognizers/country_specific/)

### Evaluation Benchmark: AI4Privacy pii-masking-300k
Use as ground truth for measuring detection quality.
- 300k annotated entries, 6 languages, 8 jurisdictions
- 98.3% token label accuracy, human-in-the-loop validated
- Includes FinPII-80k subset for financial/insurance entities
- Run claudovka pipeline (Tier 1 + Tier 2) against this dataset
- Report F1, precision, recall per entity type and per locale

Source: https://huggingface.co/datasets/ai4privacy/pii-masking-300k

### Integration Test Fixtures: PANORAMA
Use for end-to-end testing with culturally consistent synthetic profiles.
- Synthetic PII profiles across 8 locales (US, CA, UK, IE, IN, PH, NZ, AU)
- Culturally consistent names, national IDs, quasi-identifiers
- Generate test scenarios: full request bodies with embedded PII
- Verify round-trip: detect -> replace -> LLM response -> reverse replace

Source: https://arxiv.org/html/2505.12238v1

### CLI for Benchmarking
```bash
claudovka benchmark                    # Full benchmark against AI4Privacy
claudovka benchmark --locale us        # US locale only  
claudovka benchmark --locale de        # German locale only
claudovka benchmark --tier 1           # Regex only
claudovka benchmark --tier 1,2         # Regex + GLiNER
claudovka benchmark --tier 1,2,3       # Full pipeline including LLM sidecar
claudovka benchmark --report html      # Generate HTML report
```
