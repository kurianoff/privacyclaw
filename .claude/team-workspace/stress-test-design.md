# Stress Test Design: claudovka Privacy Proxy

**Target file**: `claudovka/tests/integration/stress_test.rs`
**Entry point under test**: `claudovka::proxy::intercept::run()`
**Transport**: in-memory `tokio::io::duplex()` — no TLS, no network
**Parallelism**: `tokio::task::JoinSet`

---

## 1. Scenario Table

Each row is one `#[tokio::test]` function. All tests run in the same file.

| # | Function name | PII mode | Concurrency | Payload size | SSE depth | PII density | Key assertion |
|---|---|---|---|---|---|---|---|
| S1 | `stress_passthrough_high_concurrency` | None (passthrough) | 50 | medium (10 KB) | 20 deltas | 0 | byte-identical echo |
| S2 | `stress_passthrough_large_payload` | None | 1 | large (100 KB) | 50 deltas | 0 | byte-identical echo |
| S3 | `stress_tier1_email_phone_concurrency` | Tier1 Replace | 50 | medium | 20 deltas | 2 (email + phone) | all PII masked outbound, all restored inbound |
| S4 | `stress_tier1_high_pii_density` | Tier1 Replace | 10 | large (100 KB) | 50 deltas | 10 (mixed types) | all entities masked and restored, no leakage |
| S5 | `stress_tier1_small_payload_burst` | Tier1 Replace | 100 | small (100 B) | 3 deltas | 1 (email) | PII round-trip correct for every session |
| S6 | `stress_vault_isolation` | Tier1 Replace | 20 | small | 5 deltas | 2 per session (unique per session) | no session bleeds into another session's vault |
| S7 | `stress_tier1_tier3_concurrency` | Tier1 + Tier3 (mock SLM) | 20 | medium | 20 deltas | 2 | SLM called, PII masked and restored |
| S8 | `stress_tier3_standalone_concurrency` | T3 standalone (mock SLM) | 20 | medium | 20 deltas | 2 | § tokens produced, vault populated, restored |
| S9 | `stress_split_token_many_sessions` | Tier1 Replace | 50 | small | 10 deltas | 1 (token split at `@`) | split synthetic correctly re-assembled inbound |
| S10 | `stress_mixed_modes_parallel` | None + Tier1 interleaved | 50 total (25 each) | medium | 10 deltas | 1 (PII sessions) | passthrough sessions unaffected by PII sessions running concurrently |

**Tier 2 note**: `ort-ner` feature is not enabled in default `cargo test`. Any test that would require Tier 2 is excluded. Scenarios S7 and S8 use Tier 3 via a mock HTTP server, with Tier 2 (`pipeline.tier2 = None`) disabled.

---

## 2. Helper Infrastructure

### 2.1 Shared imports and module declaration

```rust
// claudovka/tests/integration/stress_test.rs
use claudovka::dashboard::WsEvent;
use claudovka::pii::{Locale, PiiContext, PiiMode, PiiPipeline};
use claudovka::pii::vault::VaultRegistry;
use claudovka::proxy::intercept;
use claudovka::storage::Store;
use std::sync::Arc;
use std::time::Duration;
use tempfile::tempdir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::broadcast;
use tokio::task::JoinSet;
```

### 2.2 Context factories

```rust
fn make_store() -> (Store, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    (store, dir)
}

fn make_pii_ctx_tier1() -> Arc<PiiContext> {
    Arc::new(PiiContext {
        registry: Arc::new(VaultRegistry::new(Duration::from_secs(3600))),
        locale: Locale::EnUs,
        mode: PiiMode::Replace,
        pipeline: PiiPipeline::tier1_only(),
    })
}

/// Build a PiiContext with Tier 1 + Tier 3 (mock SLM at given port).
/// Tier 3 slm_confidence_threshold is set to 0.0 so all T1 spans are sent to SLM.
fn make_pii_ctx_tier1_tier3(slm_port: u16) -> Arc<PiiContext> {
    let mut cfg = claudovka::config::PiiConfig::default();
    cfg.tiers.slm = true;
    cfg.slm.endpoint = format!("http://127.0.0.1:{}", slm_port);
    cfg.slm.timeout_ms = 5000;
    cfg.slm.confidence_threshold = 0.0; // send all T1 spans to SLM
    Arc::new(PiiContext {
        registry: Arc::new(VaultRegistry::new(Duration::from_secs(3600))),
        locale: Locale::EnUs,
        mode: PiiMode::Replace,
        pipeline: PiiPipeline::new(&cfg),
    })
}

/// Build a PiiContext in T3 standalone mode (SLM only, no Tier 1/2).
fn make_pii_ctx_t3_standalone(slm_port: u16) -> Arc<PiiContext> {
    let mut cfg = claudovka::config::PiiConfig::default();
    cfg.tiers.regex = false;
    cfg.tiers.ner = false;
    cfg.tiers.slm = true;
    cfg.slm.endpoint = format!("http://127.0.0.1:{}", slm_port);
    cfg.slm.timeout_ms = 5000;
    Arc::new(PiiContext {
        registry: Arc::new(VaultRegistry::new(Duration::from_secs(3600))),
        locale: Locale::EnUs,
        mode: PiiMode::Replace,
        pipeline: PiiPipeline::new(&cfg),
    })
}
```

### 2.3 PII corpus generator

Produces strings containing known-detectable PII in Tier 1. Each call returns a unique set of values so sessions cannot accidentally share mappings.

```rust
struct PiiCorpus {
    email: String,
    phone: String,
    ssn: String,
    credit_card: String,
}

impl PiiCorpus {
    /// Generate a corpus for session `id` (1-based).
    /// Values are deterministically derived from `id` so they are always valid
    /// for Tier 1 detection and unique per session.
    fn for_session(id: usize) -> Self {
        // Email: always matches Tier 1 email regex.
        let email = format!("user{}@stress-test-corp.com", id);
        // Phone: matches US phone pattern "NXX-NXX-XXXX" where N is 2-9.
        // Use a 3-digit area code derived from id (200 + id % 800 to stay in 200-999).
        let area = 200 + (id % 800);
        let exchange = 200 + ((id * 7) % 800);
        let subscriber = 1000 + (id * 13) % 9000;
        let phone = format!("{:03}-{:03}-{:04}", area, exchange, subscriber);
        // SSN: XXX-XX-XXXX where first group is not 000/666/9XX.
        let ssn_a = 100 + (id % 500);  // 100-599, avoids 000, 666, 9XX
        let ssn_b = 10 + (id % 90);    // 10-99, avoids 00
        let ssn_c = 1000 + (id * 11) % 9000;
        let ssn = format!("{:03}-{:02}-{:04}", ssn_a, ssn_b, ssn_c);
        // Credit card: Visa (starts with 4), Luhn-valid.
        // Use a fixed Luhn-valid Visa number prefix and vary the last 4 digits.
        // 4111-1111-1111-XXXX — last 4 vary; we'll accept any for the corpus
        // (Tier 1 will detect it regardless of Luhn in stress tests; we use a known-valid
        // base number and replace only the routing portion so Luhn holds).
        // For simplicity, use the canonical test number: 4111111111111111.
        // In stress tests we don't need unique credit cards per session; they all map to one vault entry.
        let credit_card = "4111-1111-1111-1111".to_string();
        Self { email, phone, ssn, credit_card }
    }

    /// Build a message body string with `density` PII entities embedded in prose.
    /// density 1 → email only; 2 → email + phone; 3 → email + phone + ssn; 4 → all four.
    fn message(&self, density: usize) -> String {
        let mut parts = Vec::new();
        parts.push(format!("Please contact me at {} for any questions.", self.email));
        if density >= 2 {
            parts.push(format!("You can also reach me by phone at {}.", self.phone));
        }
        if density >= 3 {
            parts.push(format!("For identity verification my SSN is {}.", self.ssn));
        }
        if density >= 4 {
            parts.push(format!("My credit card number is {}.", self.credit_card));
        }
        parts.join(" ")
    }
}
```

### 2.4 Large SSE builder

Builds an Anthropic SSE response that splits a text string into `n_deltas` delta events. Used to stress the `ReplacementBuffer` across many chunk boundaries.

```rust
/// Build an Anthropic SSE response that splits `text` into `n_deltas` consecutive
/// `content_block_delta` events of roughly equal size.
fn anthropic_sse_response_n_deltas(text: &str, n_deltas: usize) -> Vec<u8> {
    assert!(n_deltas >= 1);
    let bytes = text.as_bytes();
    let chunk_size = (bytes.len() + n_deltas - 1) / n_deltas; // ceiling division

    let msg_start = serde_json::json!({
        "type": "message_start",
        "message": {
            "id": "msg_stress",
            "type": "message",
            "role": "assistant",
            "content": [],
            "model": "claude-3-5-haiku-20241022",
            "stop_reason": null,
            "stop_sequence": null,
            "usage": {"input_tokens": 10, "output_tokens": 0}
        }
    }).to_string();

    let cbs = serde_json::json!({
        "type": "content_block_start",
        "index": 0,
        "content_block": {"type": "text", "text": ""}
    }).to_string();

    let mut events = format!(
        "event: message_start\ndata: {msg_start}\n\n\
         event: content_block_start\ndata: {cbs}\n\n"
    );

    // Split text at valid UTF-8 character boundaries.
    let chars: Vec<char> = text.chars().collect();
    let chars_per_chunk = (chars.len() + n_deltas - 1) / n_deltas;
    for i in 0..n_deltas {
        let start = i * chars_per_chunk;
        if start >= chars.len() { break; }
        let end = ((i + 1) * chars_per_chunk).min(chars.len());
        let chunk: String = chars[start..end].iter().collect();
        let delta = serde_json::json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "text_delta", "text": chunk}
        }).to_string();
        events.push_str(&format!("event: content_block_delta\ndata: {delta}\n\n"));
    }

    let cbe = serde_json::json!({"type": "content_block_stop", "index": 0}).to_string();
    let msg_delta = serde_json::json!({
        "type": "message_delta",
        "delta": {"stop_reason": "end_turn", "stop_sequence": null},
        "usage": {"output_tokens": 10}
    }).to_string();
    let msg_stop = serde_json::json!({"type": "message_stop"}).to_string();

    events.push_str(&format!(
        "event: content_block_stop\ndata: {cbe}\n\n\
         event: message_delta\ndata: {msg_delta}\n\n\
         event: message_stop\ndata: {msg_stop}\n\n"
    ));

    format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n{events}").into_bytes()
}

/// Build a large (100 KB) prose filler string that does NOT contain PII.
/// Used as padding around PII entities to produce large payloads.
fn large_filler(target_bytes: usize) -> String {
    let unit = "The quick brown fox jumped over the lazy dog. ";
    let repeats = (target_bytes / unit.len()) + 1;
    unit.repeat(repeats)[..target_bytes].to_string()
}

/// Build a medium-size (10 KB) filler.
fn medium_filler() -> String {
    large_filler(10 * 1024)
}
```

### 2.5 Request builder

```rust
/// Build an Anthropic HTTP/1.1 POST request for `intercept::run` to consume.
/// `extra_content` is appended after the PII-bearing portion.
fn anthropic_request_with_content(content: &str) -> Vec<u8> {
    let body = serde_json::json!({
        "model": "claude-3-5-haiku-20241022",
        "max_tokens": 1024,
        "messages": [{"role": "user", "content": content}]
    }).to_string();
    format!(
        "POST /v1/messages HTTP/1.1\r\nHost: api.anthropic.com\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        body.len(), body
    ).into_bytes()
}
```

### 2.6 Response reader and SSE text extractor

Copy these verbatim from `pii_proxy_intercept_test.rs` (they are not exported) or move them to `tests/integration/helpers.rs` and use `mod helpers` in both files.

**Recommended action**: extract the following four functions into `claudovka/tests/integration/helpers.rs` and `mod helpers;` in each integration test file:

- `read_full_response()` — reads until `0\r\n\r\n` (chunked SSE) or Content-Length satisfied
- `read_forwarded_request_content()` — reads the proxy-forwarded request and extracts message content
- `collect_sse_text()` — parses SSE events and concatenates `text_delta` text fields
- `decode_chunked_or_raw()` — decodes chunked transfer encoding or returns raw

If extraction is not done, duplicate these four functions at the top of `stress_test.rs`.

### 2.7 Mock upstream task builder

Wrap the upstream mock into a reusable async function so each session in a JoinSet can spawn one:

```rust
/// Spawn a mock upstream task for one session.
///
/// Reads the forwarded request, verifies outbound masking of `original_pii`
/// strings, builds an SSE response echoing the masked content, and writes it back.
///
/// Returns a JoinHandle. Callers must await it after the proxy session finishes.
///
/// `upstream_io` — the upstream half of `tokio::io::duplex()` that the proxy writes to.
/// `original_pii` — list of original PII strings that must NOT appear in the forwarded request.
/// `n_deltas` — number of SSE delta events to split the echo response into.
fn spawn_upstream_mock(
    upstream_io: tokio::io::DuplexStream,
    original_pii: Vec<String>,
    n_deltas: usize,
) -> tokio::task::JoinHandle<()>
```

The function body:

1. Split `upstream_io` into `(upstream_r, upstream_w)`.
2. Call `read_forwarded_request_content(&mut upstream_r, 0).await` to get the masked content.
3. For each string in `original_pii`, assert it is absent from the masked content.
4. Build `anthropic_sse_response_n_deltas(&masked_content, n_deltas)` so the response echoes the masked synthetics back.
5. Write the SSE response to `upstream_w` and shut down.

For passthrough tests (`original_pii` is empty), the mock simply echoes the request body back unchanged in an SSE response.

---

## 3. Mock SLM Protocol

The SLM mock is a `tokio::net::TcpListener` HTTP server implementing OpenAI-compatible `/v1/chat/completions`.

### 3.1 Wire protocol

The proxy (`SlmSidecar`) sends:

```http
POST /v1/chat/completions HTTP/1.1
Host: 127.0.0.1:<port>
Content-Type: application/json
Content-Length: <n>

{
  "model": "local",
  "messages": [
    {"role": "system", "content": "<SYSTEM_PROMPT>"},
    {"role": "user",   "content": "<text or disambiguation prompt>"}
  ],
  "max_tokens": <256 for disambiguate, computed for detect_and_rewrite>,
  "temperature": 0.0
}
```

The mock must respond:

```http
HTTP/1.1 200 OK
Content-Type: application/json
Content-Length: <n>

{"choices":[{"message":{"role":"assistant","content":"<content>"}}]}
```

### 3.2 Disambiguation mode (Tier 1 + Tier 3)

The system prompt contains `"You are a PII detection validator"`. The response `content` must be a JSON array of integer indices, e.g. `"[0, 1]"` to confirm all candidates, or `"[]"` to reject all.

For stress tests: **confirm all candidates** by returning `"[0,1,2,...,n-1]"` where `n` is the number of candidates in the request. The simplest implementation reads the request body, counts the number of `[<digit>]` lines in the prompt, and returns the full index array.

### 3.3 T3 standalone mode

The system prompt contains `"You are a PII redactor"`. The user message contains the original text. The response `content` must be the rewritten text with `§value§` tokens wrapping PII.

For stress tests, the mock must:
1. Parse the user message from the request JSON.
2. Replace known PII strings with `§<original>§` markers.
3. Return the rewritten text as response content.

Because the stress tests use the corpus generator with known PII patterns (email format `userN@stress-test-corp.com`), the mock can apply a simple regex to identify and wrap those values. The simplest correct implementation: scan the user message for any string matching the pattern `user\d+@stress-test-corp\.com` and wrap it as `§<match>§`.

### 3.4 Multi-connection mock SLM server

Unlike the unit-test helpers in `tier3.rs` (which accept one connection), stress tests require a server that handles N concurrent connections for N parallel sessions.

```rust
/// Start a mock SLM server that handles up to `max_connections` requests.
///
/// `mode` controls the response logic:
///   - `SlmMockMode::ConfirmAll` — returns JSON array confirming all candidates (T1+T3 mode)
///   - `SlmMockMode::StandaloneRewrite` — rewrites text, wrapping stress-corpus emails in § (T3 standalone)
///
/// Returns the port the server is listening on.
async fn start_mock_slm_server(mode: SlmMockMode, max_connections: usize) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        let mut count = 0;
        while count < max_connections {
            match listener.accept().await {
                Ok((mut stream, _)) => {
                    count += 1;
                    let mode = mode.clone();
                    tokio::spawn(async move {
                        handle_slm_connection(&mut stream, mode).await;
                    });
                }
                Err(_) => break,
            }
        }
    });

    // Give the listener task time to start.
    tokio::time::sleep(Duration::from_millis(5)).await;
    port
}

#[derive(Clone)]
enum SlmMockMode {
    ConfirmAll,
    StandaloneRewrite,
}

async fn handle_slm_connection(
    stream: &mut tokio::net::TcpStream,
    mode: SlmMockMode,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // Read until blank line + Content-Length body.
    let mut buf = vec![0u8; 65536];
    let n = stream.read(&mut buf).await.unwrap_or(0);
    let raw = &buf[..n];

    // Find body.
    let body_start = raw.windows(4).position(|w| w == b"\r\n\r\n")
        .map(|p| p + 4)
        .unwrap_or(n);
    let body = &raw[body_start..];
    let req_json: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();

    let content = match mode {
        SlmMockMode::ConfirmAll => {
            // Count candidate spans from the prompt.
            // The disambiguation prompt contains "[N]" lines for each candidate.
            let user_content = req_json["messages"]
                .get(1)
                .and_then(|m| m["content"].as_str())
                .unwrap_or("");
            // Count lines matching "[<digits>]"
            let n_candidates = user_content
                .lines()
                .filter(|l| l.trim().starts_with('[')
                    && l.trim().chars().nth(1).map(|c| c.is_ascii_digit()).unwrap_or(false))
                .count();
            let indices: Vec<String> = (0..n_candidates).map(|i| i.to_string()).collect();
            format!("[{}]", indices.join(","))
        }
        SlmMockMode::StandaloneRewrite => {
            // Rewrite: wrap stress-corpus emails in § markers.
            let user_content = req_json["messages"]
                .get(1)
                .and_then(|m| m["content"].as_str())
                .unwrap_or("")
                .to_string();
            // Replace "userN@stress-test-corp.com" patterns.
            let re = regex::Regex::new(r"user\d+@stress-test-corp\.com").unwrap();
            re.replace_all(&user_content, |caps: &regex::Captures| {
                format!("§{}§", &caps[0])
            }).to_string()
        }
    };

    let response_body = format!(
        r#"{{"choices":[{{"message":{{"role":"assistant","content":"{}"}}}}]}}"#,
        content.replace('"', "\\\"")
    );
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        response_body.len(), response_body
    );
    let _ = stream.write_all(response.as_bytes()).await;
}
```

**Important**: for T3 standalone mode the content string can contain `§` characters which need JSON escaping. Use `serde_json::json!` for serialization rather than raw string formatting, or use `serde_json::to_string(&content)` to produce a properly escaped JSON string value.

---

## 4. Correctness Invariants

These are the precise assertions each scenario must make after every session completes.

### 4.1 Passthrough invariant (S1, S2, S10 passthrough sessions)

```
assert!(!forwarded_request_content.is_empty())
assert!(forwarded_request_content == original_content)  // no modification
assert!(response_text == original_content)              // no modification
```

Specifically: the original message content (including any email-like strings if present) must appear verbatim in both the forwarded request and the decoded response.

### 4.2 PII masking invariant (S3–S9 outbound leg, verified in upstream mock)

For each original PII value `pii_v` in `original_pii`:
```
assert!(!forwarded_request_content.contains(pii_v),
    "PII must not appear in forwarded request: {pii_v} found in {forwarded_request_content}")
```

Additionally: for Tier 1, the synthetic is a valid alternative value of the same type (email for email, etc.). The test does not need to verify synthetic type — only that the original is absent.

### 4.3 PII restoration invariant (S3–S9 inbound leg, verified in client response)

For each original PII value `pii_v`:
```
let decoded = collect_sse_text(&response_bytes);
assert!(decoded.contains(pii_v),
    "Original PII must be restored in response: {pii_v} not in {decoded}")
```

### 4.4 Vault isolation invariant (S6)

Each session `i` uses a corpus with unique PII (e.g. `user1@...` vs `user2@...`). After all sessions complete:
- Extract the synthetic for session `i`'s email from the forwarded request.
- Assert that synthetic does NOT appear in any other session's response.

Implementation: use `Arc<Mutex<HashMap<usize, String>>>` to collect `(session_id → synthetic)` from all upstream mock tasks, then verify cross-session isolation after the JoinSet finishes.

### 4.5 Byte integrity invariant (S1, S2 passthrough)

For non-PII payloads, the proxy must forward every byte unchanged:
```
assert_eq!(forwarded_body_bytes.len(), original_body_bytes.len())
assert_eq!(&forwarded_body_bytes[..], &original_body_bytes[..])
```

This requires the upstream mock to capture the raw body bytes and compare them. Use `read_forwarded_request_content` extended to also return the raw body, or write a separate `read_forwarded_raw_body()` helper.

### 4.6 No panic / no task abort invariant (all scenarios)

Every `JoinHandle` must be awaited and checked:
```rust
let result = join_set.join_next().await.unwrap(); // Option<JoinHandle result>
result.expect("session task panicked");
```

`intercept::run()` returns `Result<()>`; the test must call `.unwrap()` on the inner result too.

### 4.7 Timing invariant (all scenarios)

Wrap each parallel session group in `tokio::time::timeout`:
```rust
tokio::time::timeout(Duration::from_secs(60), async move {
    // ... JoinSet completion loop
}).await.expect("stress test timed out")
```

Use 60 seconds for Tier 3 scenarios, 30 seconds for Tier 1 and passthrough.

---

## 5. Concurrency Harness

### 5.1 JoinSet-based parallel session pattern

This is the standard pattern for all concurrent scenarios. Each parallel "session" consists of three co-operating async tasks:
1. `upstream_task` — mock upstream HTTP server
2. `intercept_task` — `intercept::run()`
3. The driving code that writes the request and reads the response

Because the driving code (writing and reading) must happen in sequence (write first, then read), but each session is independent of other sessions, the JoinSet manages sessions, and within each session the tasks are sequential per the lifecycle documented in `pii_proxy_intercept_test.rs`.

```rust
async fn run_n_sessions(
    n: usize,
    pii_ctx: Option<Arc<PiiContext>>,  // None for passthrough
    make_content: impl Fn(usize) -> String + Send + Sync + 'static,
    n_deltas: usize,
) -> Vec<String> {  // returns decoded response texts per session
    let (store, _dir) = make_store();
    let results: Arc<Mutex<Vec<(usize, String)>>> = Default::default();

    let make_content = Arc::new(make_content);
    let mut join_set = JoinSet::new();

    for session_id in 0..n {
        let store = store.clone();
        let pii_ctx = pii_ctx.clone();
        let make_content = Arc::clone(&make_content);
        let results = Arc::clone(&results);

        join_set.spawn(async move {
            let (ws_tx, _) = broadcast::channel::<WsEvent>(8);
            let content = make_content(session_id);
            let request_bytes = anthropic_request_with_content(&content);

            // Duplex streams simulating client <-> proxy <-> upstream
            let buf_size = 512 * 1024; // 512 KB per stream, large enough for 100 KB payloads
            let (client_io, proxy_client_io) = tokio::io::duplex(buf_size);
            let (proxy_upstream_io, upstream_io) = tokio::io::duplex(buf_size);

            let (proxy_client_r, proxy_client_w) = tokio::io::split(proxy_client_io);
            let (proxy_upstream_r, proxy_upstream_w) = tokio::io::split(proxy_upstream_io);
            let (mut client_r, mut client_w) = tokio::io::split(client_io);

            // Original PII values: if PII mode active, extract from content.
            let pii_values: Vec<String> = if pii_ctx.is_some() {
                // Caller must provide these; the harness can also accept a closure.
                // For simplicity, pass via a closure parameter in real implementation.
                vec![]
            } else {
                vec![]
            };

            let upstream_task = spawn_upstream_mock(upstream_io, pii_values, n_deltas);

            let intercept_task = tokio::spawn(intercept::run(
                proxy_client_r, proxy_client_w,
                proxy_upstream_r, proxy_upstream_w,
                "api.anthropic.com".to_string(),
                store, ws_tx, pii_ctx,
            ));

            client_w.write_all(&request_bytes).await.unwrap();
            let response = read_full_response(&mut client_r).await;
            client_w.shutdown().await.unwrap();

            intercept_task.await.unwrap().unwrap();
            upstream_task.await.unwrap();

            let decoded = collect_sse_text(&response);
            results.lock().unwrap().push((session_id, decoded));
        });
    }

    // Drain all sessions.
    while let Some(res) = join_set.join_next().await {
        res.expect("session panicked");
    }

    // Return results ordered by session_id.
    let mut v = results.lock().unwrap().clone();
    v.sort_by_key(|(id, _)| *id);
    v.into_iter().map(|(_, text)| text).collect()
}
```

**Note on duplex buffer size**: `tokio::io::duplex(n)` creates a bounded channel of `n` bytes. For 100 KB payloads with SSE overhead, use at least `512 * 1024` (512 KB). For small payloads, `128 * 1024` is sufficient.

### 5.2 Session-scoped PII tracking for vault isolation (S6)

To verify vault isolation, the harness must capture which synthetic was assigned to each session's email. Do this inside the upstream mock:

```rust
// Session-scoped result: (session_id, synthetic_used, original_pii)
type IsolationMap = Arc<Mutex<HashMap<usize, (String, String)>>>;

// In the upstream mock for session_id:
let masked_content = read_forwarded_request_content(&mut upstream_r, 0).await;
let original_email = corpus.email.clone();
assert!(!masked_content.contains(&original_email));
// Extract synthetic by removing the known prefix.
let synthetic = masked_content
    .trim_start_matches("Please contact me at ")
    .split_whitespace()
    .next()
    .unwrap_or("")
    .to_string();
isolation_map.lock().unwrap().insert(session_id, (synthetic, original_email));
```

After all sessions complete, iterate `isolation_map` and assert no two sessions share the same synthetic value (emails from different sessions, having different local parts, will always produce different synthetics because `SyntheticGenerator` derives them from the original).

---

## 6. Detailed Scenario Specifications

### S1: `stress_passthrough_high_concurrency`

- 50 concurrent sessions
- Each session: 10 KB content (medium_filler()), no PII
- Upstream mock echoes content verbatim in SSE with 20 deltas
- Per session assertion: `decoded_response == original_content`
- Global assertion: all 50 sessions complete without panic
- Timeout: 30 seconds

### S2: `stress_passthrough_large_payload`

- 1 session
- 100 KB content body (large_filler(100_000))
- Upstream mock echoes verbatim with 50 deltas
- Assertion: byte-identical content in response
- Also assert forwarded body length == original body length
- Timeout: 30 seconds

### S3: `stress_tier1_email_phone_concurrency`

- 50 concurrent sessions, each with unique corpus (session_id)
- Content: `corpus.message(2)` (email + phone)
- Upstream mock: asserts neither email nor phone appear in forwarded request; echoes entire masked content in SSE (20 deltas)
- Per session: `decoded.contains(corpus.email)` and `decoded.contains(corpus.phone)`
- No session bleeds into another (email format unique per session)
- Timeout: 30 seconds

### S4: `stress_tier1_high_pii_density`

- 10 concurrent sessions
- Content: `corpus.message(4)` (all 4 PII types) prepended with 90 KB of filler to make payload ~100 KB
- Upstream mock echoes masked content with 50 deltas
- Per session: assert all 4 originals are absent outbound and all 4 are present in decoded response
- Timeout: 30 seconds

### S5: `stress_tier1_small_payload_burst`

- 100 concurrent sessions
- Content: `corpus.message(1)` (~60 bytes + email)
- SSE: 3 deltas
- Per session: email absent outbound, email present in decoded response
- Timeout: 30 seconds

### S6: `stress_vault_isolation`

- 20 concurrent sessions
- Each session uses a unique corpus (email = `userN@stress-test-corp.com`)
- Shared `IsolationMap` (Arc<Mutex<HashMap<session_id, synthetic>>>)
- After all sessions finish: for each pair `(i, j)` where `i != j`, assert `synthetic_i != synthetic_j`
  - This relies on the fact that different originals produce different synthetics
- Also: no session's decoded response contains another session's original email
- Timeout: 30 seconds

### S7: `stress_tier1_tier3_concurrency`

- Start mock SLM with `SlmMockMode::ConfirmAll`, `max_connections = 20`
- 20 concurrent sessions, Tier1+Tier3 PiiContext
- Content: `corpus.message(2)` (email + phone)
- Mock SLM confirms all candidates (both email and phone are confirmed PII)
- Per session: email absent outbound, email present in decoded response
- Timeout: 60 seconds (SLM adds latency)

### S8: `stress_tier3_standalone_concurrency`

- Start mock SLM with `SlmMockMode::StandaloneRewrite`, `max_connections = 20`
- 20 concurrent sessions, T3 standalone PiiContext
- Content: `corpus.message(1)` (email only, since standalone SLM only wraps known email pattern)
- Upstream mock: asserts masked content contains `§userN@stress-test-corp.com§` and not the bare email; echoes § token in SSE
- Per session: decoded response contains original email (§ token restored)
- Timeout: 60 seconds

### S9: `stress_split_token_many_sessions`

- 50 concurrent sessions, Tier1 Replace
- Content: `"contact user{N}@stress-test-corp.com for billing"`
- Upstream mock: reads masked content, extracts synthetic email, splits at `@`, builds split SSE response using `anthropic_sse_response_n_deltas` where the first half of synthetic goes in delta 1 and the second half in delta 2 through delta 10
- Per session: decoded response contains original email (split token reassembled)
- Timeout: 30 seconds

Implementation detail for splitting: after extracting the synthetic from the masked content, compute split point at the `@` character. If `@` not found (shouldn't happen for email synthetics), split at `len/2`. Then build `n_deltas = 10` response where the text echoed is `"your email: {synthetic}"` and the SSE builder naturally splits it across 10 chunks.

### S10: `stress_mixed_modes_parallel`

- 25 passthrough sessions + 25 PII sessions, all 50 running concurrently
- Use the same shared `Store` (one `tempdir`)
- Passthrough sessions use `pii_ctx = None`
- PII sessions use `make_pii_ctx_tier1()` (each gets its own `VaultRegistry`)
- Passthrough assertion: content unchanged
- PII assertion: email masked outbound, restored inbound
- Global assertion: passthrough sessions' decoded text does NOT contain synthetic tokens
- Timeout: 30 seconds

---

## 7. File Structure

```
claudovka/tests/
  integration/
    helpers.rs                        -- shared helpers extracted from pii_proxy_intercept_test.rs
    pii_proxy_intercept_test.rs       -- existing (updated to use helpers.rs)
    pii_roundtrip_test.rs             -- existing
    passthrough_no_pii_test.rs        -- existing
    multiturn_consistency_test.rs     -- existing
    vault_persistence_test.rs         -- existing
    stress_test.rs                    -- NEW (this design)
  e2e/
    helpers.rs                        -- existing
    init_test.rs                      -- existing
    proxy_lifecycle_test.rs           -- existing
    network_proxy_test.rs             -- existing
```

The `helpers.rs` extraction enables `stress_test.rs` to call `read_full_response`, `collect_sse_text`, `decode_chunked_or_raw`, and `read_forwarded_request_content` without duplication.

To expose `tests/integration/helpers.rs` to both test files, each integration test file must declare:

```rust
#[path = "helpers.rs"]
mod helpers;
use helpers::*;
```

Or use a shared module declaration in `tests/integration/mod.rs` if the test runner supports it (Cargo supports module files in integration test directories via `tests/<name>/main.rs` style, but `#[path]` is simpler).

---

## 8. Implementation Notes for the Developer

### 8.1 duplex buffer sizing

`tokio::io::duplex(buf_size)` must be large enough to hold the full SSE response before the reader drains it, otherwise the upstream mock task will block on write and deadlock. For 100 KB payloads with SSE framing overhead (each delta event adds ~120 bytes of JSON envelope), use:
- Small scenarios (100 B): `128 * 1024`
- Medium scenarios (10 KB): `256 * 1024`
- Large scenarios (100 KB): `1024 * 1024` (1 MB)

### 8.2 JoinSet vs join_all

Use `tokio::task::JoinSet` rather than `futures::future::join_all`. `JoinSet` allows collecting results one by one as they complete and propagates panics correctly via the `JoinError` variant.

### 8.3 Store sharing

In scenarios that use multiple sessions with the same `PiiContext` (and thus the same `VaultRegistry`), the `Store` must also be shared so `get_or_create_with_store` can persist and reload vaults. Use `store.clone()` (Store wraps `Arc<Mutex<Connection>>`).

However, for vault isolation tests (S6), all sessions share one registry and one store. The isolation property is enforced by unique conversation IDs generated by `intercept::run` from the JSON body's `conversation_id` field (or a UUID if absent). Since the stress test uses the default `intercept::run` path (which generates a UUID per session), each session gets a distinct vault automatically.

### 8.4 Test timeout constants

The test file should define:
```rust
const PASSTHROUGH_TIMEOUT: Duration = Duration::from_secs(30);
const PII_TIMEOUT: Duration = Duration::from_secs(30);
const T3_TIMEOUT: Duration = Duration::from_secs(60);
```

These wrap the entire JoinSet drain loop, not individual sessions.

### 8.5 UPSTREAM_READ_TIMEOUT in tests

`intercept.rs` sets `UPSTREAM_READ_TIMEOUT = Duration::from_millis(2000)` when compiled with `#[cfg(test)]`. This means if the upstream mock takes more than 2 seconds to respond, the proxy will abort with a timeout warning. The mock SLM server must respond within 2 seconds. For T3 scenarios, the mock SLM should write the response immediately after reading the request (no artificial delay).

### 8.6 Cargo.toml additions

No new dependencies are required. The `regex` crate is already in `[dependencies]` and available in `[dev-dependencies]` via the workspace. Confirm that `tempfile` is in `[dev-dependencies]` (it is, based on existing tests using it).

Check `[dev-dependencies]` for:
```toml
tempfile = "3"
tracing-test = "0.2"  # optional, for log assertions
```

### 8.7 PiiContext per-session vs shared

For scenarios S3–S5, S9 (vault isolation not under test): use a single `Arc<PiiContext>` shared across all sessions. The `VaultRegistry` inside it is shared, but since each session generates a unique conversation ID (UUID), each session gets its own vault slot in the registry. This correctly mirrors production behavior.

For scenario S6 (vault isolation under test): same architecture, but explicitly verify cross-session vault separation as described in section 6.

### 8.8 Known limitation: `collect_sse_text` on chunked responses

The proxy wraps SSE responses in chunked transfer encoding when in PII replace mode. `collect_sse_text` calls `decode_chunked_or_raw` before parsing SSE events. In passthrough mode, the response is forwarded verbatim (no chunked re-encoding). Ensure the response parser handles both cases.

For passthrough tests, the upstream mock returns a non-chunked Content-Length response. For PII tests, the proxy re-encodes as chunked. `collect_sse_text` + `decode_chunked_or_raw` handles both paths.

---

## 9. Cross-Reference: Existing Test Patterns

The following patterns from `pii_proxy_intercept_test.rs` are **directly reusable** in `stress_test.rs`:

| Pattern | Source location | Used in scenario |
|---|---|---|
| `make_pii_context()` | line 34 | Replicated as `make_pii_ctx_tier1()` |
| `make_store()` | line 43 | All scenarios |
| `anthropic_request()` | line 50 | Replaced by `anthropic_request_with_content()` |
| `anthropic_sse_response()` | line 89 | Used internally by `spawn_upstream_mock()` |
| `anthropic_sse_response_split()` | line 120 | S9 (generalized to `anthropic_sse_response_n_deltas()`) |
| `read_full_response()` | line 154 | All scenarios (move to helpers.rs) |
| `read_forwarded_request_content()` | line 199 | All PII scenarios (move to helpers.rs) |
| `collect_sse_text()` | line 237 | All scenarios (move to helpers.rs) |
| `decode_chunked_or_raw()` | line 266 | All scenarios (move to helpers.rs) |
| Test lifecycle (write → spawn → read → shutdown) | line 315 | All scenarios |

The mock SLM pattern from `tier3.rs` tests (`test_disambiguate_mock_server_confirms_subset`, line 510) is the foundation for `handle_slm_connection()` in section 3.4.

The T3 standalone mock (`mock_slm_server()`) from `t3_standalone_roundtrip.rs` (line 22) is the foundation for `start_mock_slm_server(SlmMockMode::StandaloneRewrite, ...)`.
