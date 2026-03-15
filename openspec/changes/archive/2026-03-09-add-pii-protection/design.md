# Design: PII Detection and Bidirectional Replacement

## Context

privacyclaw Phase 1 is a zero-latency MITM proxy: bytes are forwarded immediately while parsing happens off-path on a tee'd copy. Phase 2 changes this contract for the **outbound (client→upstream) path only**: the request body must be fully buffered, PII-detected, and potentially rewritten before forwarding. The **inbound (upstream→client) path** retains near-zero latency via a prefix-aware streaming buffer.

The key asymmetry (from the task spec):
- Outbound: full JSON body available before any byte goes to the LLM. Buffer → detect → replace → forward.
- Inbound: SSE streaming. Only reverse lookups against the vault. No detection; cheap O(text_length × max_key_len) scan.

## Goals / Non-Goals

Goals:
- Zero real PII leaves the machine to LLM APIs when PII mode is enabled.
- LLM response is indistinguishable from a direct response (synthetic tokens reversed back).
- Tier 1+2 adds <100ms to outbound request path.
- Inbound adds <5ms per SSE chunk.
- Works fully with Tier 1 only (no ML models required for basic operation).
- Multi-turn conversations use consistent mappings (same input → same synthetic output).
- Vault persists across proxy restarts (crash safety).

Non-Goals:
- Phase 2 does NOT handle image/binary content (future phase).
- Phase 2 does NOT redact PII from logs/storage (stored content includes original PII locally — it is only sanitised before sending to LLMs).
- Phase 2 does NOT support real-time Vault editing via the UI.
- Phase 2 does NOT guarantee 100% PII recall (Tier 1+2 targets 90%+ for common types).

## Architecture

```
intercept.rs outbound path (handle_c2u):

  [READ full request body]
        |
        v
  [PiiPipeline::process_request(body, vault)]
        |    Tier 1: regex (<2ms)
        |    Tier 2: GLiNER ONNX (~20-50ms) [if enabled]
        |    Tier 3: SLM sidecar (~100-500ms) [if enabled]
        |
        v
  [Rewrite JSON: replace PII in all message.content fields]
  [Update Content-Length header]
        |
        v
  [Forward modified request to upstream]
  [Log sanitised content to dashboard/storage]


intercept.rs inbound path (handle_u2c):

  [SSE chunk arrives from upstream]
        |
        v
  [Forward chunk bytes to client immediately — BEFORE any processing]
  (ReplacementBuffer intercepts the TEXT DELTA extracted from SSE envelope,
   not the raw bytes. Raw bytes always flow through unmodified.)
        |
        v
  Wait — Phase 2 changes this: the SSE text content displayed in the
  dashboard and stored in the log is the DE-ANONYMISED version.
  The bytes forwarded to the client are the ORIGINAL from upstream
  (which contain synthetic tokens), NOT reversed.

CORRECTION: The inbound path reversal is only for the CLIENT-FACING stream.
  - Bytes forwarded to client: REVERSED (synthetic → original)
  - Bytes stored in DB / shown in dashboard: reversed (original PII visible locally)

The SSE envelope wrapping the text is reconstructed after replacement.
This means we CANNOT forward raw bytes — we must:
  1. Parse SSE chunk to extract text delta
  2. Replace synthetic → original in text delta
  3. Re-wrap in SSE JSON envelope
  4. Forward re-wrapped bytes to client

This is a significant change: Phase 1 was byte-identical passthrough.
Phase 2 modifies SSE response bytes (text content only, envelope structure preserved).
```

## Key Data Structures

### PiiVault

```rust
pub struct PiiVault {
    // original → synthetic (used on outbound, keyed by original string)
    pub original_to_synthetic: HashMap<String, String>,
    // synthetic → original (used on inbound, keyed by synthetic string)
    pub synthetic_to_original: HashMap<String, String>,
    // Aho-Corasick over all synthetic keys — for fast multi-pattern replace
    pub reverse_automaton: AhoCorasick,
    // Max length of any synthetic key — used for buffer window sizing
    pub max_synthetic_key_len: usize,
    // Seeded RNG for deterministic synthetic generation per conversation
    // Seed = sha256(conversation_id)[0..8] as u64
    pub rng_seed: u64,
    pub conversation_id: String,
    pub created_at: DateTime<Utc>,
}

// Thread-safe handle
pub type VaultHandle = Arc<RwLock<PiiVault>>;

// Registry: one vault per active conversation
pub struct VaultRegistry {
    vaults: Mutex<HashMap<String, VaultHandle>>,
    ttl: Duration,
}
```

### PII Detection Result

```rust
pub struct PiiSpan {
    pub start: usize,       // byte offset in original text
    pub end: usize,
    pub entity_type: PiiType,
    pub confidence: f32,
    pub tier: u8,           // 1=regex, 2=gliner, 3=slm
}

pub enum PiiType {
    Email, Phone, Ssn, CreditCard, IpV4, IpV6,
    ApiKey, AwsAccessKey, AwsSecretKey, GitHubToken,
    BearerToken, SshPrivateKey, DbConnectionString, UrlWithCreds,
    PersonName, OrgName, Address, DateOfBirth, MedicalRecord,
    PassportNumber, DriversLicense, FinancialAccount,
    Custom(String),
}
```

### ReplacementBuffer (inbound SSE)

```rust
pub struct ReplacementBuffer {
    vault: VaultHandle,
    buffer: String,
}

impl ReplacementBuffer {
    /// Process incoming text delta from SSE envelope.
    /// Returns text to flush to client (with synthetic→original replacements applied).
    /// May buffer a trailing window to avoid splitting multi-token synthetic values.
    pub fn process_delta(&mut self, incoming: &str) -> String;

    /// Flush all remaining buffered text at end of stream.
    pub fn flush_remaining(&mut self) -> String;
}
```

## Integration Points in intercept.rs

`intercept::run` gains two new parameters:

```rust
pub async fn run(
    client_reader: ...,
    client_writer: ...,
    upstream_reader: ...,
    upstream_writer: ...,
    host: String,
    store: Store,
    ws_tx: broadcast::Sender<WsEvent>,
    vault_registry: Arc<VaultRegistry>,  // NEW
    pii_config: Arc<PiiConfig>,          // NEW
) -> Result<()>
```

**handle_c2u changes**:
1. Do NOT write chunks to upstream immediately.
2. Accumulate complete request (headers + body).
3. Once body is complete: run `PiiPipeline::process_request()`.
4. Rebuild HTTP request with modified body + updated Content-Length.
5. Write modified request to upstream.

**handle_u2c changes**:
1. Forward raw bytes to client immediately (unchanged from Phase 1).

Wait — there's a conflict. If we forward raw SSE bytes directly, the client sees synthetic tokens. We must instead parse→replace→re-wrap. This changes the byte-stream the client receives.

**Revised handle_u2c**:
1. Buffer SSE chunks per-event (parse the SSE envelope).
2. For text delta events: apply ReplacementBuffer to the text content.
3. Re-wrap the modified text delta in the original SSE JSON structure.
4. Forward re-wrapped bytes to client.
5. For non-text events (message_start, message_stop, etc.): forward as-is.

This means the client receives structurally identical SSE events but with text deltas having synthetic tokens replaced. The SSE envelope format is preserved.

## Decisions

### Decision: Outbound path latency budget
- **Decision**: Accept up to 100ms added latency for Tier 1+2. Gate Tier 3 behind explicit flag.
- **Rationale**: LLM API calls are already slow (1-5s TTFT). 100ms is imperceptible.
- **Alternative considered**: Async parallel detection while forwarding bytes. Rejected because we must know the full text before we can replace it consistently.

### Decision: Aho-Corasick for reverse replacement
- **Decision**: Use `aho-corasick 1.1` with `MatchKind::LeftmostLongest` for the vault reverse automaton.
- **Rationale**: The vault may contain overlapping synthetic tokens (e.g., "Alice" and "Alice Brown"). Leftmost-longest ensures consistent behaviour.
- **Alternative**: String `.replace()` in a loop. Rejected: O(N×K) for N text length and K keys; also can double-replace.

### Decision: ReplacementBuffer window size
- **Decision**: Hold back `max_synthetic_key_len` bytes at the tail of the buffer, but only if the tail begins with a character that appears as the first char of any synthetic key.
- **Rationale**: Minimises buffering (near-zero extra latency in practice, since synthetic tokens are multi-word strings that rarely split exactly at an SSE chunk boundary).
- **Alternative**: Always hold back max_synthetic_key_len bytes. Simpler but adds constant latency.

### Decision: SSE text delta re-wrapping
- **Decision**: Parse the SSE envelope to extract text delta, apply replacement, re-wrap using the same JSON structure (preserving all other fields).
- **Rationale**: Correct client experience. The LLM response the user sees must contain original PII (the replacement was only for the LLM's benefit).
- **Constraint**: The re-wrapped event may differ in byte length from the original. Transfer-Encoding: chunked handles this correctly (chunk size prefix is per-chunk, not per-stream). Content-Length is N/A for SSE (it uses chunked or keep-alive with event-stream).

### Decision: GLiNER as optional Cargo feature
- **Decision**: Tier 2 (GLiNER) behind `ort-ner` Cargo feature. Not compiled by default.
- **Rationale**: `ort` adds significant build complexity and binary size (~100MB ONNX runtime). Users who only want Tier 1 regex should not pay this cost.
- **Alternative**: Always compile, conditional on config. Rejected: compile time and binary size impact.

### Decision: Seeded deterministic RNG per conversation
- **Decision**: Seed = `u64::from_le_bytes(sha256(conversation_id)[..8])`. Store seed in vault, re-create `SmallRng` from seed when adding new mappings.
- **Rationale**: Ensures that the same input PII always gets the same synthetic replacement within a conversation, even across proxy restarts (vault reloaded from storage).
- **Alternative**: UUID-based replacement (fixed lookup table). Less natural-looking.

### Decision: Vault persistence format
- **Decision**: Persist vault as an additional NDJSON line in the conversation file with `"type":"vault"` discriminator. On conversation load, read this line to restore mappings.
- **Rationale**: Keeps storage simple (no new file type, no schema migration). The vault line comes immediately after the conversation header.
- **Alternative**: Separate `.vault.json` file per conversation. Cleaner separation but doubles file count.

### Decision: PII mode default
- **Decision**: `pii.mode = "off"` by default. Must be explicitly enabled.
- **Rationale**: Phase 1 users get no behaviour change when upgrading. PII mode is opt-in.

## Risks / Trade-offs

- **SSE re-wrapping latency**: Re-parsing and re-constructing SSE JSON adds ~0.1ms per event. Acceptable.
- **Missed PII**: Tier 1+2 won't catch 100% of PII. This is documented behaviour for Phase 2 (Phase 3 adds context-aware SLM).
- **Double-encoding edge cases**: If LLM echoes back a synthetic token that also appears as a real word (e.g., "Alice"), we reverse it everywhere in the response — potentially incorrectly. Mitigated by choosing less-common synthetic names.
- **Content-Length mismatch**: If PII replacement changes body size, Content-Length MUST be updated. Failure to do so will corrupt the upstream TLS connection. This must be robustly tested.
- **Keep-alive multi-turn**: The vault must survive across multiple HTTP requests on the same TCP connection. The vault is keyed by conversation_id which is determined at request parse time, so this is handled by the VaultRegistry.

## Migration Plan

- Phase 1 config files are fully compatible. The new `[pii]` section defaults to `mode = "off"`.
- No storage migration required: new vault NDJSON lines are ignored by Phase 1 readers.
- Binary is backwards-compatible: `privacyclaw start` without `--pii` flag behaves identically to Phase 1.

## Open Questions

- Should the stored conversation log contain the original PII or the synthetic version? Currently proposed: original (local log shows real data; only LLM sees sanitised version). This means the log itself contains PII and must be treated as sensitive.
- Should Tier 2 (GLiNER) run synchronously on the hot path, or in a background task with a timeout fallback? Recommendation: synchronous with a 500ms timeout; if timeout, log warning and proceed with Tier 1 results only.
- What is the right buffer eviction policy for the VaultRegistry? Proposed: LRU + TTL (default 24h), max 1000 active sessions.
